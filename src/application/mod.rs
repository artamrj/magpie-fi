pub mod apply;
pub mod artwork;
pub mod canonical_names;
pub mod files;
pub mod input_dedup;
pub mod library;
pub mod metadata_completion;
pub mod scan;
pub mod smart_approval;

use crate::{app::AppState, infrastructure::providers::Candidate};
use anyhow::Result;
use metadata_completion::CompletionReport;

/// The catalog rows that may complete a selected recording, and the album the
/// file already names.
pub struct Donors<'a> {
    pub candidates: &'a [Candidate],
    pub existing_album: Option<&'a str>,
}

/// Finishes a selected recording before it can become Ready: completes it from
/// agreeing catalog rows (when `donors` are given), applies canonical names,
/// runs the cover-art worker and audits the result.
pub async fn finalize_selection(
    state: &AppState,
    candidate: &mut Candidate,
    donors: Option<Donors<'_>>,
) -> Result<CompletionReport> {
    if let Some(donors) = donors {
        metadata_completion::complete(candidate, donors.candidates, donors.existing_album);
    }
    canonical_names::canonicalize_candidates(&state.pool, std::slice::from_mut(candidate)).await?;
    {
        let _permit = state.artwork_downloads.acquire().await?;
        artwork::ensure_usable_cover(&state.pool, &state.client, candidate).await;
    }
    Ok(metadata_completion::reassess(candidate))
}
