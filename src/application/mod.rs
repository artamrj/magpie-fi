pub mod apply;
pub mod artist_changes;
pub mod artists;
pub mod artwork;
pub mod canonical_names;
pub mod files;
pub mod input_dedup;
pub mod library;
pub mod metadata_completion;
pub mod scan;
pub mod smart_approval;
pub mod tidy;

use crate::domain::credits;
use crate::{app::AppState, infrastructure::providers::Candidate};
use anyhow::Result;
use metadata_completion::CompletionReport;

/// The catalog rows that may complete a selected recording, and the album the
/// file already names.
pub struct Donors<'a> {
    pub candidates: &'a [Candidate],
    pub existing_album: Option<&'a str>,
}

/// What the file itself says, from its tags or its name.
#[derive(Clone, Copy, Debug, Default)]
pub struct FileTags<'a> {
    pub artist: Option<&'a str>,
    pub title: Option<&'a str>,
}

/// Sources disagree about who is the main singer of a duet or a featured song: Radio Javan,
/// YouTube or a release by the featured artist list that artist first. The file's own tag
/// decides. When it names a main singer the match credits in another position, that singer
/// goes first, the others become featured, and the album artist follows (a single's album
/// artist is its main singer; a compilation is left alone).
pub fn keep_file_main_singer(candidate: &mut Candidate, file: FileTags<'_>) {
    let Some(main) = file
        .artist
        .and_then(|artist| credits::main_name(artist, file.title.unwrap_or_default()))
    else {
        return;
    };
    let artists = if candidate.artist_credits.is_empty() {
        credits::normalize_featured(&candidate.artist, &candidate.title).artists
    } else {
        std::mem::take(&mut candidate.artist_credits)
    };
    let (artists, moved) = credits::keep_main_first(artists, &main);
    candidate.artist_credits = artists;
    if !moved {
        return;
    }
    let before = candidate.artist.clone();
    candidate.artist = credits::display_artist(&candidate.artist_credits);
    let primary = candidate.artist_credits[0].clone();
    let album_artist_is_a_singer = candidate
        .album_artist
        .as_deref()
        .and_then(|value| credits::main_name(value, ""))
        .is_some_and(|name| {
            candidate
                .artist_credits
                .iter()
                .any(|credit| credits::identity_key(&credit.name) == credits::identity_key(&name))
        });
    if album_artist_is_a_singer {
        candidate.album_artist = Some(primary.name.clone());
        candidate.album_artist_credits = vec![credits::ArtistCredit {
            join_phrase: String::new(),
            ..primary
        }];
    }
    tracing::info!(
        from = before,
        to = candidate.artist,
        "kept the file's main singer first"
    );
}

/// Finishes a selected recording before it can become Ready: completes it from
/// agreeing catalog rows (when `donors` are given), applies canonical names,
/// runs the cover-art worker and audits the result.
pub async fn finalize_selection(
    state: &AppState,
    candidate: &mut Candidate,
    donors: Option<Donors<'_>>,
    file: Option<FileTags<'_>>,
) -> Result<CompletionReport> {
    if let Some(file) = file {
        keep_file_main_singer(candidate, file);
    }
    if let Some(donors) = donors {
        metadata_completion::complete(candidate, donors.candidates, donors.existing_album);
    }
    canonical_names::canonicalize_candidates(
        &state.pool,
        None,
        std::slice::from_mut(candidate),
        artists::Mode::Lookup,
    )
    .await?;
    {
        let _permit = state.artwork_downloads.acquire().await?;
        artwork::ensure_usable_cover(&state.pool, &state.client, candidate).await;
    }
    Ok(metadata_completion::reassess(candidate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_decides_who_is_the_main_singer() {
        // A release by the featured artist lists her first; the file says Tqvis is main.
        let mut candidate = Candidate {
            artist: "Ferap".into(),
            title: "Donyaye Movazi (feat. Tqvis)".into(),
            album_artist: Some("Ferap".into()),
            ..Default::default()
        };
        keep_file_main_singer(
            &mut candidate,
            FileTags {
                artist: Some("Tqvis & Ferap"),
                title: Some("Donyaye Movazi"),
            },
        );
        assert_eq!(candidate.artist, "Tqvis feat. Ferap");
        assert_eq!(candidate.album_artist.as_deref(), Some("Tqvis"));
        assert_eq!(candidate.album_artist_credits.len(), 1);

        // A compilation's album artist is not a singer and stays.
        let mut compilation = Candidate {
            artist: "Ferap feat. Tqvis".into(),
            album_artist: Some("Various Artists".into()),
            ..Default::default()
        };
        keep_file_main_singer(
            &mut compilation,
            FileTags {
                artist: Some("Tqvis"),
                title: None,
            },
        );
        assert_eq!(compilation.artist, "Tqvis feat. Ferap");
        assert_eq!(compilation.album_artist.as_deref(), Some("Various Artists"));

        // A match by someone else entirely is left alone.
        let mut other = Candidate {
            artist: "Maria".into(),
            ..Default::default()
        };
        keep_file_main_singer(
            &mut other,
            FileTags {
                artist: Some("Tqvis"),
                title: None,
            },
        );
        assert_eq!(other.artist, "Maria");
    }
}
