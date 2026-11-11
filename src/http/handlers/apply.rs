use super::*;
use crate::application::apply::{self as apply_service, Claim};

pub async fn start_apply(State(s): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    // Preparing can take minutes (new artists are looked up one per second),
    // longer than the page waits for an answer. It runs on its own task and
    // starts the filing itself, so the workflow slot is never left taken by a
    // request that was dropped half-way.
    let starting = tokio::spawn({
        let s = s.clone();
        async move {
            let claim = apply_service::claim_ready(&s).await?;
            Ok::<_, anyhow::Error>(match claim {
                Claim::Ready(prepared) => {
                    let apply_service::PreparedApply {
                        items,
                        selected_count,
                        outputs,
                        duplicates_skipped,
                    } = prepared;
                    tokio::spawn(apply_service::run_claimed(s.clone(), items));
                    Ok((selected_count, outputs, duplicates_skipped))
                }
                Claim::Busy => Err(ApiError::conflict("workflow is already running")),
                Claim::NothingReady => Err(ApiError::validation(
                    "No identified songs are ready to file",
                )),
            })
        }
    });
    let (selected_count, outputs, duplicates_skipped) = match starting.await {
        Ok(started) => started??,
        Err(error) => {
            s.reset_workflow(WorkflowPhase::Idle, "Ready").await;
            return Err(ApiError::Internal(anyhow::anyhow!(
                "filing could not start: {error}"
            )));
        }
    };
    Ok(Json(serde_json::json!({
        "started": true,
        "count": selected_count,
        "outputs": outputs,
        "duplicates_skipped": duplicates_skipped
    })))
}
