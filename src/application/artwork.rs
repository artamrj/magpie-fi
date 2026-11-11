//! Cover-art worker.
//!
//! This is the single owner of cover art for the whole pipeline. A track is
//! only ever written when its artwork has been verified to be a real, usable
//! image that matches the selected recording, and the same guarantee is
//! enforced for outputs that are *reused* instead of rewritten.
//!
//! The worker gathers artwork from every source that is allowed for the
//! selected recording — the candidate's own URL list, agreeing catalog rows,
//! a fresh catalog search when the stored URLs are unusable, and artwork that
//! was already verified for the same release elsewhere in the library (so the
//! first track of an album helps fill in every other track). When none of
//! those works, it looks for the cover of the album itself: by album name and
//! artist on Apple Music and Deezer, and other editions of the same MusicBrainz
//! album. The chosen release and its tags never change to get a cover.
//! Every candidate image is downloaded and inspected before it is accepted.

use crate::infrastructure::providers::{
    ArtworkCandidate, ArtworkStatus, Candidate, cover_art_archive, musicbrainz::is_mbid,
};
use anyhow::{Result, bail};
use reqwest::Client;
use serde_json::Value;
use sqlx::SqlitePool;
use std::collections::HashSet;

use crate::application::metadata_completion::{albums_compatible, same_recording, text_similarity};
use crate::infrastructure::media::tag_writer::ArtworkInfo;

/// Every artwork URL that is allowed for this recording, in a sensible order:
/// user-confirmed and release-backed URLs first, then agreeing catalog rows,
/// then artwork already verified for the same release/isrc in the library.
/// URLs are deduplicated. Nothing here downloads anything.
pub async fn collect_artwork_candidates(
    pool: &SqlitePool,
    candidate: &Candidate,
) -> Result<Vec<ArtworkCandidate>> {
    let mut artwork = candidate.artwork_candidates.clone();
    if let Some(url) = candidate.cover_url.as_deref() {
        push_new(
            &mut artwork,
            ArtworkCandidate {
                provider: candidate.provider.clone(),
                url: url.to_owned(),
                release_id: candidate.release_id.clone(),
                isrc: candidate.isrc.clone(),
                album: candidate.album.clone(),
                artist: Some(candidate.artist.clone()),
                ..Default::default()
            },
        );
    }
    for library in library_verified_artwork(pool, candidate).await? {
        push_new(&mut artwork, library);
    }
    Ok(artwork)
}

fn push_new(artwork: &mut Vec<ArtworkCandidate>, item: ArtworkCandidate) {
    if !artwork.iter().any(|existing| existing.url == item.url) {
        artwork.push(item);
    }
}

/// A verified artwork row from another candidate that shares the same release.
#[derive(sqlx::FromRow)]
struct VerifiedArtworkRow {
    cover_url: Option<String>,
    provider: Option<String>,
    isrc: Option<String>,
    album: Option<String>,
    artist: Option<String>,
}

/// Reuse artwork that was already verified for the same release or ISRC
/// elsewhere in the library. The bytes themselves are shared through the
/// artwork-url provider cache, so this makes the first verified cover of an
/// album instantly available to every other track on the same release.
/// Release ids are only compared when they are MusicBrainz ids: other
/// catalogs' ids are not unique across catalogs.
async fn library_verified_artwork(
    pool: &SqlitePool,
    candidate: &Candidate,
) -> Result<Vec<ArtworkCandidate>> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    if let Some(release_id) = candidate.release_id.as_deref().filter(|id| is_mbid(id)) {
        let rows: Vec<VerifiedArtworkRow> = sqlx::query_as(
            "SELECT cover_url, provider, isrc, album, artist
             FROM candidates
             WHERE artwork_status='verified' AND musicbrainz_release_id=? AND cover_url IS NOT NULL",
        )
        .bind(release_id)
        .fetch_all(pool)
        .await?;
        for row in rows {
            push_library_row(&mut out, &mut seen, candidate, row);
        }
    }
    if let Some(isrc) = candidate.isrc.as_deref() {
        let rows: Vec<VerifiedArtworkRow> = sqlx::query_as(
            "SELECT cover_url, provider, isrc, album, artist
             FROM candidates
             WHERE artwork_status='verified' AND isrc=? COLLATE NOCASE AND cover_url IS NOT NULL",
        )
        .bind(isrc)
        .fetch_all(pool)
        .await?;
        for row in rows {
            push_library_row(&mut out, &mut seen, candidate, row);
        }
    }
    Ok(out)
}

fn push_library_row(
    out: &mut Vec<ArtworkCandidate>,
    seen: &mut HashSet<String>,
    candidate: &Candidate,
    row: VerifiedArtworkRow,
) {
    let VerifiedArtworkRow {
        cover_url,
        provider,
        isrc,
        album,
        artist,
    } = row;
    if let Some(url) = cover_url
        && seen.insert(url.clone())
    {
        out.push(ArtworkCandidate {
            provider: provider.unwrap_or_else(|| "Library".into()),
            url,
            release_id: candidate.release_id.clone(),
            isrc: isrc.or_else(|| candidate.isrc.clone()),
            album,
            artist: artist.or_else(|| Some(candidate.artist.clone())),
            ..Default::default()
        });
    }
}

/// Run the cover-art worker for a selected recording. Tries every matching
/// catalog URL (and artwork already verified for the same release), then
/// falls back to a fresh iTunes/Deezer lookup. Updates the candidate's
/// artwork status.
pub async fn ensure_usable_cover(pool: &SqlitePool, client: &Client, candidate: &mut Candidate) {
    candidate.artwork_status = ArtworkStatus::Searching;
    candidate.artwork_message = Some("Checking matching catalog artwork".into());
    let mut artwork = match collect_artwork_candidates(pool, candidate).await {
        Ok(artwork) => artwork,
        Err(error) => {
            tracing::warn!(?error, "could not collect artwork candidates");
            Vec::new()
        }
    };
    let mut check = verify_artwork_candidates(pool, client, candidate, &artwork).await;
    if let Some((item, _, info)) = check.verified {
        accept_verified_artwork(candidate, artwork, &item, &info);
        return;
    }
    if check.retryable {
        mark_retryable_artwork(candidate, artwork, check.failures);
        return;
    }
    let initial_count = artwork.len();

    // Existing release-backed URLs were unusable. Refresh only now, using the
    // selected recording identity, so healthy cached artwork stays instant.
    let (deezer_isrc, itunes, deezer) = tokio::join!(
        async {
            if let Some(isrc) = candidate.isrc.as_deref() {
                crate::infrastructure::providers::deezer::lookup_isrc(pool, client, isrc).await
            } else {
                Ok(None)
            }
        },
        crate::infrastructure::providers::itunes::search(
            pool,
            client,
            &candidate.title,
            Some(&candidate.artist),
            candidate.album.as_deref(),
        ),
        crate::infrastructure::providers::deezer::search(
            pool,
            client,
            &candidate.title,
            Some(&candidate.artist),
        )
    );
    let mut sources = Vec::new();
    match deezer_isrc {
        Ok(found) => sources.extend(found),
        Err(error) => check.retryable |= is_retryable_artwork_error(&error),
    }
    // A song filed as its own single has no album to compare: the cover of the
    // release this same recording is on is its cover (what every store shows).
    let own_title = is_own_title_release(candidate);
    let same_album =
        |source: &Candidate| albums_compatible(candidate.album.as_deref(), source.album.as_deref());
    for result in [itunes, deezer] {
        match result {
            Ok(found) => sources.extend(found.into_iter().map(without_length).filter(|source| {
                same_recording(candidate, source)
                    && (same_album(source) || (own_title && !source.is_compilation))
            })),
            Err(error) => check.retryable |= is_retryable_artwork_error(&error),
        }
    }
    // The same release first, then a single of this song ("What Was I Made For? (From
    // …) - Single"), then the album the song is on.
    let song = crate::domain::names::match_key(&candidate.title);
    let named_after_song = |source: &Candidate| {
        source.album.as_deref().is_some_and(|album| {
            !song.is_empty() && crate::domain::names::match_key(album).starts_with(&song)
        })
    };
    sources.sort_by_key(|source| (!same_album(source), !named_after_song(source)));
    for source in sources {
        if let Some(url) = source.cover_url {
            push_new(
                &mut artwork,
                ArtworkCandidate {
                    provider: source.provider,
                    url,
                    same_recording: true,
                    release_id: source.release_id,
                    isrc: source.isrc,
                    album: source.album,
                    artist: Some(source.artist),
                    ..Default::default()
                },
            );
        }
    }
    let recovery =
        verify_artwork_candidates(pool, client, candidate, &artwork[initial_count..]).await;
    check.retryable |= recovery.retryable;
    check.failures.extend(recovery.failures);
    if let Some((item, _, info)) = recovery.verified {
        accept_verified_artwork(candidate, artwork, &item, &info);
        return;
    }

    // Nothing found through the song. Look for the album's own cover.
    let album_start = artwork.len();
    let (album_sources, album_retryable) = album_artwork(pool, client, candidate).await;
    check.retryable |= album_retryable;
    for item in album_sources {
        push_new(&mut artwork, item);
    }
    let album_check =
        verify_artwork_candidates(pool, client, candidate, &artwork[album_start..]).await;
    check.retryable |= album_check.retryable;
    check.failures.extend(album_check.failures);
    if let Some((item, _, info)) = album_check.verified {
        accept_verified_artwork(candidate, artwork, &item, &info);
        return;
    }
    if check.retryable {
        mark_retryable_artwork(candidate, artwork, check.failures);
        return;
    }
    candidate.cover_url = None;
    candidate.artwork_candidates = artwork;
    candidate.artwork_status = ArtworkStatus::CoverRequired;
    candidate.artwork_message = Some(if check.failures.is_empty() {
        "No matching catalog artwork was found; provide a cover URL".to_owned()
    } else {
        format!("No usable matching cover: {}", check.failures.join("; "))
    });
    mark_cover_verified(candidate, false, "cover_required");
}

/// Covers of the chosen album found by the album, not the song: the release
/// group's front cover (any edition of the same MusicBrainz album) and album
/// searches by name and artist. Each is still checked against the album and
/// artist by [`artwork_matches_candidate`]. Also says whether a source failed
/// in a way worth retrying.
async fn album_artwork(
    pool: &SqlitePool,
    client: &Client,
    candidate: &Candidate,
) -> (Vec<ArtworkCandidate>, bool) {
    let mut out = Vec::new();
    let mut retryable = false;
    if let Some(group) = candidate
        .release_group_id
        .as_deref()
        .filter(|id| is_mbid(id))
    {
        // A release group is this album by MusicBrainz ID, so its cover is
        // labelled with the chosen album and artist.
        out.push(ArtworkCandidate {
            provider: "Cover Art Archive (album)".into(),
            url: cover_art_archive::release_group_front_url(group),
            album: candidate.album.clone(),
            artist: Some(candidate.artist.clone()),
            ..Default::default()
        });
    }
    let Some(album) = candidate
        .album
        .as_deref()
        .filter(|album| !album.trim().is_empty())
    else {
        return (out, retryable);
    };
    let artist = candidate
        .album_artist
        .as_deref()
        .filter(|artist| !artist.trim().is_empty() && !is_various_artists(artist))
        .unwrap_or(&candidate.artist);
    let (itunes, deezer) = tokio::join!(
        crate::infrastructure::providers::itunes::album_covers(pool, client, artist, album),
        crate::infrastructure::providers::deezer::album_covers(pool, client, artist, album),
    );
    for result in [itunes, deezer] {
        match result {
            Ok(found) => out.extend(found),
            Err(error) => retryable |= is_retryable_artwork_error(&error),
        }
    }
    (out, retryable)
}

/// A row fresh from a store search carries the song's whole length where a scanned match
/// carries its difference from the file. The file's length is not known here, so the row
/// is compared without one; read as a difference, every row looked minutes off and no
/// cover was ever found this way.
fn without_length(mut row: Candidate) -> Candidate {
    row.duration_delta = None;
    row
}

/// The song is filed as its own single: its album is only its own title, because no
/// source named an album or the match is a single. A named album or a version in the
/// album name ("… (Live at …)") is a release of its own and keeps the strict check.
fn is_own_title_release(candidate: &Candidate) -> bool {
    let title = crate::domain::credits::release_title_without_featured(&candidate.title);
    candidate.album.as_deref().is_some_and(|album| {
        crate::domain::names::album_key(album) == crate::domain::names::album_key(&title)
    })
}

fn is_various_artists(artist: &str) -> bool {
    matches!(
        artist.trim().to_lowercase().as_str(),
        "various artists" | "various" | "va"
    )
}

/// Apply-time: return the exact bytes to embed, or fail so the track is
/// returned to review instead of being written without matching artwork.
/// Only artwork that matches the selected release is considered.
pub async fn resolve_for_write(
    pool: &SqlitePool,
    client: &Client,
    candidate: &Candidate,
) -> Result<Vec<u8>> {
    let artwork = collect_artwork_candidates(pool, candidate).await?;
    let check = verify_artwork_candidates(pool, client, candidate, &artwork).await;
    if let Some((_, bytes, _)) = check.verified {
        return Ok(bytes);
    }
    bail!(
        "no usable matching cover could be downloaded{}",
        if check.failures.is_empty() {
            String::new()
        } else {
            format!(": {}", check.failures.join("; "))
        }
    )
}

fn mark_retryable_artwork(
    candidate: &mut Candidate,
    artwork: Vec<ArtworkCandidate>,
    failures: Vec<String>,
) {
    if candidate.cover_url.is_none() {
        candidate.cover_url = artwork.first().map(|item| item.url.clone());
    }
    candidate.artwork_candidates = artwork;
    candidate.artwork_status = ArtworkStatus::RetryableError;
    candidate.artwork_message =
        Some("Cover download temporarily unavailable; retry will use the selected URL".into());
    record_artwork_failures(candidate, &failures);
    mark_cover_verified(candidate, false, "retryable_error");
}

fn record_artwork_failures(candidate: &mut Candidate, failures: &[String]) {
    let mut breakdown = candidate
        .score_breakdown
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    breakdown["metadata_completion"]["artwork_failures"] = serde_json::json!(failures);
    candidate.score_breakdown = Some(breakdown.to_string());
}

/// The first usable matching cover among `artwork`, with what went wrong for
/// the others.
struct ArtworkCheck {
    verified: Option<(ArtworkCandidate, Vec<u8>, ArtworkInfo)>,
    retryable: bool,
    failures: Vec<String>,
}

async fn verify_artwork_candidates(
    pool: &SqlitePool,
    client: &Client,
    candidate: &Candidate,
    artwork: &[ArtworkCandidate],
) -> ArtworkCheck {
    let mut check = ArtworkCheck {
        verified: None,
        retryable: false,
        failures: Vec::new(),
    };
    for item in artwork {
        if !artwork_matches_candidate(candidate, item) {
            check.failures.push(format!(
                "{} did not match the selected release",
                item.provider
            ));
            continue;
        }
        let trusted_exact = item.user_confirmed
            || item.own_record
            || item
                .release_id
                .as_deref()
                .zip(candidate.release_id.as_deref())
                .is_some_and(|(artwork_release, selected_release)| {
                    artwork_release == selected_release
                });
        match cover_art_archive::fetch_verified_url_cached(pool, client, &item.url, trusted_exact)
            .await
        {
            Ok((bytes, info)) => {
                check.verified = Some((item.clone(), bytes, info));
                return check;
            }
            Err(error) => {
                check.retryable |= is_retryable_artwork_error(&error);
                check.failures.push(format!("{}: {error:#}", item.provider));
            }
        }
    }
    check
}

fn accept_verified_artwork(
    candidate: &mut Candidate,
    artwork: Vec<ArtworkCandidate>,
    selected: &ArtworkCandidate,
    info: &ArtworkInfo,
) {
    candidate.cover_url = Some(selected.url.clone());
    candidate.artwork_candidates = artwork;
    candidate.artwork_status = ArtworkStatus::Verified;
    candidate.artwork_message = Some(format!(
        "Verified {}x{} {} cover from {}",
        info.width, info.height, info.mime, selected.provider
    ));
    mark_cover_verified(candidate, true, &selected.provider);
}

fn mark_cover_verified(candidate: &mut Candidate, verified: bool, source: &str) {
    let mut breakdown = candidate
        .score_breakdown
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    breakdown["metadata_completion"]["cover_verified"] = serde_json::json!(verified);
    breakdown["metadata_completion"]["cover_source"] = serde_json::json!(source);
    // When the last search came up empty, so the cover finder waits before the next one.
    breakdown["metadata_completion"]["cover_searched_at"] = if verified {
        Value::Null
    } else {
        serde_json::json!(chrono::Utc::now().to_rfc3339())
    };
    candidate.score_breakdown = Some(breakdown.to_string());
}

/// A catalog artwork candidate may only be used for a recording it actually
/// belongs to. User-confirmed and exact-release URLs are always accepted.
fn artwork_matches_candidate(candidate: &Candidate, artwork: &ArtworkCandidate) -> bool {
    if artwork.user_confirmed || artwork.own_record {
        return true;
    }
    if artwork
        .release_id
        .as_deref()
        .zip(candidate.release_id.as_deref())
        .is_some_and(|(left, right)| left == right)
    {
        return true;
    }
    if artwork
        .isrc
        .as_deref()
        .zip(candidate.isrc.as_deref())
        .is_some_and(|(left, right)| left.eq_ignore_ascii_case(right))
    {
        let release_compatible = candidate
            .release_type
            .as_deref()
            .is_some_and(|kind| kind.eq_ignore_ascii_case("single"))
            || albums_compatible(artwork.album.as_deref(), candidate.album.as_deref());
        return release_compatible
            && artwork
                .artist
                .as_deref()
                .is_none_or(|artist| text_similarity(artist, &candidate.artist) >= 0.82);
    }
    // An album is listed under its album artist ("ANOTR" for a song by
    // "ANOTR feat. 54 Ultra"), so either name counts.
    let same_artist = artwork.artist.as_deref().is_some_and(|artist| {
        std::iter::once(candidate.artist.as_str())
            .chain(candidate.album_artist.as_deref())
            .any(|name| text_similarity(artist, name) >= 0.82)
    });
    if artwork.same_recording && is_own_title_release(candidate) {
        return same_artist;
    }
    // Entries are only created for rows that already agreed on the recording
    // (same title), so a row without an album name only needs the artist.
    same_artist
        && match artwork.album.as_deref() {
            Some(album) => albums_compatible(Some(album), candidate.album.as_deref()),
            None => true,
        }
}

pub(crate) fn is_retryable_artwork_error(error: &anyhow::Error) -> bool {
    error.chain().any(|source| {
        source
            .downcast_ref::<reqwest::Error>()
            .is_some_and(|error| {
                error.is_timeout()
                    || error.is_connect()
                    || error
                        .status()
                        .is_some_and(|status| status.as_u16() == 429 || status.is_server_error())
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::providers::Candidate;

    fn candidate(provider: &str, album: Option<&str>) -> Candidate {
        Candidate {
            provider: provider.into(),
            title: "Farangis".into(),
            artist: "Siavash Ghomayshi".into(),
            album: album.map(str::to_owned),
            duration_delta: Some(1.0),
            score: 90.0,
            ..Default::default()
        }
    }

    /// Well-known songs whose cover every store shows. Run with
    /// `cargo test --locked covers_of_well_known_songs -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "live cover probe"]
    async fn covers_of_well_known_songs_are_found() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("covers.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        let client = Client::new();
        for (title, artist, album, kind) in [
            (
                "Nemesis",
                "Benjamin Clementine",
                Some("At Least for Now"),
                Some("album"),
            ),
            (
                "Nemesis",
                "Benjamin Clementine",
                Some("Nemesis"),
                Some("single"),
            ),
            ("Nemesis", "Benjamin Clementine", Some("Nemesis"), None),
            (
                "What Was I Made For?",
                "Billie Eilish",
                Some("What Was I Made For?"),
                Some("single"),
            ),
            (
                "What Was I Made For?",
                "Billie Eilish",
                Some("What Was I Made For? (live from Lollapalooza Chicago 2023)"),
                None,
            ),
        ] {
            let mut song = Candidate {
                provider: "musicbrainz".into(),
                title: title.into(),
                artist: artist.into(),
                album: album.map(str::to_owned),
                release_type: kind.map(str::to_owned),
                duration_delta: Some(1.0),
                ..Default::default()
            };
            ensure_usable_cover(&pool, &client, &mut song).await;
            // A live release no store carries has no cover to find; the others do.
            let in_stores = !album.is_some_and(|album| album.contains("Lollapalooza"));
            assert_eq!(
                song.artwork_status == ArtworkStatus::Verified,
                in_stores,
                "{title} / {album:?}: {:?}",
                song.artwork_message
            );
            println!(
                "{title} / {album:?} / {kind:?}\n  status={:?} cover={:?}\n  message={:?}\n  options={:?}",
                song.artwork_status,
                song.cover_url,
                song.artwork_message,
                song.artwork_candidates
                    .iter()
                    .map(|item| (
                        item.provider.clone(),
                        item.album.clone(),
                        item.artist.clone()
                    ))
                    .collect::<Vec<_>>()
            );
        }
    }

    const RELEASE: &str = "b10bbbfc-cf9e-42e0-be17-e2c3e1d2600d";

    #[test]
    fn a_song_filed_as_its_own_single_takes_the_cover_of_the_album_it_is_on() {
        // "Nemesis" is only sold on the album "At Least for Now".
        let mut song = candidate("musicbrainz", Some("Nemesis"));
        song.title = "Nemesis".into();
        song.artist = "Benjamin Clementine".into();
        let store_row = |same_recording| ArtworkCandidate {
            provider: "itunes".into(),
            url: "https://example.test/at-least-for-now.jpg".into(),
            same_recording,
            album: Some("At Least for Now".into()),
            artist: Some("Benjamin Clementine".into()),
            ..Default::default()
        };
        assert!(artwork_matches_candidate(&song, &store_row(true)));
        // An album found by name only, not through this recording, still has to match.
        assert!(!artwork_matches_candidate(&song, &store_row(false)));
        // Another artist's "Nemesis" never counts.
        let mut other = store_row(true);
        other.artist = Some("Arch Enemy".into());
        assert!(!artwork_matches_candidate(&song, &other));
        // A named release keeps the strict check: a live single does not get the studio cover.
        song.album = Some("Nemesis (Live For Burberry)".into());
        assert!(!artwork_matches_candidate(&song, &store_row(true)));
    }

    #[test]
    fn a_fresh_store_row_is_compared_without_its_length() {
        use crate::application::metadata_completion::same_recording;
        // The scanned match is 1 s off the file; the store row says the song lasts 300 s.
        let mut song = candidate("musicbrainz", Some("Farangis"));
        song.duration_delta = Some(1.0);
        let mut row = candidate("itunes", Some("Farangis"));
        row.duration_delta = Some(300.0);
        assert!(!same_recording(&song, &row));
        assert!(same_recording(&song, &without_length(row)));
    }

    #[test]
    fn confirmed_single_accepts_its_isrc_matched_catalog_cover_title() {
        let mut selected = candidate("deezer", Some("Single"));
        selected.release_type = Some("single".into());
        selected.isrc = Some("QM-TEST-123".into());
        let artwork = ArtworkCandidate {
            provider: "Deezer".into(),
            url: "https://cdn-images.dzcdn.net/cover.jpg".into(),
            isrc: Some("qm-test-123".into()),
            album: Some("Moorche (feat. Mehrad Hidden)".into()),
            artist: Some("Siavash Ghomayshi".into()),
            ..Default::default()
        };

        assert!(artwork_matches_candidate(&selected, &artwork));
        selected.release_type = Some("album".into());
        assert!(!artwork_matches_candidate(&selected, &artwork));
    }

    #[test]
    fn a_chosen_single_keeps_the_cover_its_catalog_showed() {
        // After choosing, "Farangis - Single" becomes "Farangis"; the cover rows
        // were stored with the catalog's own album name.
        let selected = candidate("itunes", Some("Farangis"));
        let catalog_row = ArtworkCandidate {
            provider: "Deezer".into(),
            url: "https://example.test/farangis.jpg".into(),
            album: Some("Farangis - Single".into()),
            artist: Some("Siavash Ghomayshi".into()),
            ..Default::default()
        };
        assert!(artwork_matches_candidate(&selected, &catalog_row));

        // Rows without an album (Navahang, Radio Javan singles) only need the artist.
        let no_album = ArtworkCandidate {
            album: None,
            ..catalog_row.clone()
        };
        assert!(artwork_matches_candidate(&selected, &no_album));
        let other_artist = ArtworkCandidate {
            artist: Some("Ebi".into()),
            ..no_album
        };
        assert!(!artwork_matches_candidate(&selected, &other_artist));

        // The match's own record always fits, whatever it calls the album.
        let own = ArtworkCandidate {
            own_record: true,
            album: Some("Something else entirely".into()),
            ..catalog_row
        };
        assert!(artwork_matches_candidate(&selected, &own));
    }

    #[test]
    fn a_different_album_is_still_refused() {
        let selected = candidate("itunes", Some("Khabe Baroon"));
        let other_album = ArtworkCandidate {
            provider: "Deezer".into(),
            url: "https://example.test/best-of.jpg".into(),
            album: Some("Greatest Hits 1990-2010".into()),
            artist: Some("Siavash Ghomayshi".into()),
            ..Default::default()
        };
        assert!(!artwork_matches_candidate(&selected, &other_album));
    }

    #[test]
    fn same_release_artwork_is_collected_from_the_library() {
        // Covered in the async tests below; this guards the release/ISRC keys.
        let mut selected = candidate("musicbrainz", Some("Khabe Baroon"));
        selected.release_id = Some("release-1".into());
        let artwork = ArtworkCandidate {
            provider: "Deezer".into(),
            url: "https://example.test/cover.jpg".into(),
            release_id: Some("release-1".into()),
            artist: Some("Siavash Ghomayshi".into()),
            ..Default::default()
        };
        assert!(artwork_matches_candidate(&selected, &artwork));
    }

    #[tokio::test]
    async fn artwork_verified_for_same_release_is_reused() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("library-artwork.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        let track_id = sqlx::query("INSERT INTO tracks(path,filename,status,is_missing,first_seen_at,last_seen_at,last_scanned_at,stage) VALUES('/music/album-track.mp3','album-track.mp3','needs_review',0,'now','now','now','review')")
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_rowid();
        sqlx::query(
            "INSERT INTO candidates(track_id,provider,title,artist,album,musicbrainz_release_id,cover_url,score,artwork_status)
             VALUES(?,'itunes','Farangis','Siavash Ghomayshi','Khabe Baroon',?,'https://example.test/verified.jpg',95,'verified')",
        )
        .bind(track_id)
        .bind(RELEASE)
        .execute(&pool)
        .await
        .unwrap();
        crate::infrastructure::provider_cache::put(
            &pool,
            "artwork-url",
            &crate::infrastructure::provider_cache::url_key("https://example.test/verified.jpg"),
            &serde_json::json!({
                "data_base64": base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    crate::infrastructure::media::tag_writer::test_artwork_png()
                )
            }),
            chrono::Utc::now() + chrono::Duration::days(1),
        )
        .await
        .unwrap();

        let mut selected = candidate("musicbrainz", Some("Khabe Baroon"));
        selected.release_id = Some(RELEASE.into());

        ensure_usable_cover(&pool, &Client::new(), &mut selected).await;
        assert_eq!(
            selected.cover_url.as_deref(),
            Some("https://example.test/verified.jpg")
        );
        assert_eq!(selected.artwork_status, ArtworkStatus::Verified);

        // Other catalogs' release ids are not unique, so they are never shared.
        sqlx::query("UPDATE candidates SET musicbrainz_release_id='1440857781'")
            .execute(&pool)
            .await
            .unwrap();
        let mut other_catalog = candidate("itunes", Some("Khabe Baroon"));
        other_catalog.release_id = Some("1440857781".into());
        assert!(
            library_verified_artwork(&pool, &other_catalog)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn broken_primary_cover_falls_back_to_a_verified_catalog_image() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("cover-worker.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        for (url, data) in [
            ("https://example.test/broken.jpg", vec![0]),
            (
                "https://example.test/valid.png",
                crate::infrastructure::media::tag_writer::test_artwork_png(),
            ),
        ] {
            crate::infrastructure::provider_cache::put(
                &pool,
                "artwork-url",
                &crate::infrastructure::provider_cache::url_key(url),
                &serde_json::json!({"data_base64": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, data)}),
                chrono::Utc::now() + chrono::Duration::days(1),
            )
            .await
            .unwrap();
        }
        let mut selected = candidate("musicbrainz", Some("Khabe Baroon"));
        selected.cover_url = Some("https://example.test/broken.jpg".into());
        selected.artwork_candidates = vec![ArtworkCandidate {
            provider: "Deezer".into(),
            url: "https://example.test/valid.png".into(),
            album: Some("Khabe Baroon".into()),
            artist: Some("Siavash Ghomayshi".into()),
            ..Default::default()
        }];

        ensure_usable_cover(&pool, &Client::new(), &mut selected).await;
        assert_eq!(selected.artwork_status, ArtworkStatus::Verified);
        assert_eq!(
            selected.cover_url.as_deref(),
            Some("https://example.test/valid.png")
        );
    }

    #[tokio::test]
    async fn temporary_cover_failure_keeps_the_selected_url_for_retry() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("cover-retry.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        let url = "http://127.0.0.1:1/temporarily-unavailable.jpg";
        let mut selected = candidate("manual", Some("Single"));
        selected.cover_url = Some(url.into());
        selected.artwork_candidates = vec![ArtworkCandidate {
            provider: "User verified".into(),
            url: url.into(),
            user_confirmed: true,
            ..Default::default()
        }];

        ensure_usable_cover(&pool, &Client::new(), &mut selected).await;

        assert_eq!(selected.cover_url.as_deref(), Some(url));
        assert_eq!(selected.artwork_status, ArtworkStatus::RetryableError);
        assert_eq!(
            selected.artwork_message.as_deref(),
            Some("Cover download temporarily unavailable; retry will use the selected URL")
        );
        assert!(
            !crate::application::metadata_completion::audit(&selected, Vec::new()).core_complete
        );
    }

    #[tokio::test]
    async fn resolve_for_write_errors_when_no_matching_cover_can_be_fetched() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("resolve-write.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        let mut selected = candidate("manual", Some("Single"));
        selected.cover_url = Some("http://127.0.0.1:1/unavailable.jpg".into());
        selected.artwork_candidates = vec![ArtworkCandidate {
            provider: "User verified".into(),
            url: "http://127.0.0.1:1/unavailable.jpg".into(),
            user_confirmed: true,
            ..Default::default()
        }];

        assert!(
            resolve_for_write(&pool, &Client::new(), &selected)
                .await
                .is_err()
        );
    }
}
