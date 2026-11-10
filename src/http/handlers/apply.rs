use super::*;
use crate::application::apply::{self as apply_service, Claim};

pub async fn start_apply(State(s): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    let prepared = match apply_service::claim_ready(&s).await? {
        Claim::Busy => return Err(ApiError::conflict("workflow is already running")),
        Claim::NothingReady => {
            return Err(ApiError::validation(
                "No identified songs are ready to file",
            ));
        }
        Claim::Ready(prepared) => prepared,
    };
    let apply_service::PreparedApply {
        items,
        selected_count,
        outputs,
        duplicates_skipped,
    } = prepared;
    tokio::spawn(apply_service::run_claimed(s.clone(), items));
    Ok(Json(serde_json::json!({
        "started": true,
        "count": selected_count,
        "outputs": outputs,
        "duplicates_skipped": duplicates_skipped
    })))
}
