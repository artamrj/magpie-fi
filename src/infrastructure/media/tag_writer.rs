use crate::infrastructure::providers::{Candidate, musicbrainz::is_mbid};
use anyhow::{Context, Result, bail};
use lofty::{
    config::{ParseOptions, WriteOptions},
    file::TaggedFileExt,
    flac::FlacFile,
    ogg::OggPictureStorage,
    picture::{Picture, PictureType},
    prelude::*,
    probe::Probe,
    tag::{ItemKey, ItemValue, Tag, TagItem, TagType},
};
use sha2::{Digest, Sha256};
use std::path::Path;

/// Cover images larger than this are refused.
pub const MAX_ARTWORK_BYTES: usize = 20 * 1024 * 1024;

/// Rewrites the tags from `candidate`, with the cover and lyrics inside the
/// file. Without new `lyrics`, lyrics already in the file are kept.
pub fn write(
    path: &Path,
    candidate: &Candidate,
    artwork: Option<Vec<u8>>,
    lyrics: Option<&str>,
) -> Result<()> {
    let mut candidate = candidate.clone();
    crate::application::metadata_completion::normalize_release_fields(&mut candidate);
    let ext = path
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(ext.as_str(), "wav" | "aiff" | "aif") {
        bail!("{ext} files are not tagged: their tag support is unreliable");
    }
    let mut file = Probe::open(path)?.guess_file_type()?.read()?;
    let preserved_artwork = valid_embedded_artwork(&file);
    let preserved_lyrics = embedded_lyrics(&file);
    // Rebuild the primary tag instead of mutating it. Real music collections often
    // contain malformed frames that can be read but cannot be saved again.
    file.insert_tag(Tag::new(file.primary_tag_type()));
    let tag = file.primary_tag_mut().expect("primary tag inserted");
    let credits = crate::domain::credits::normalize_structured(
        &candidate.artist,
        &candidate.title,
        std::mem::take(&mut candidate.artist_credits),
    );
    candidate.artist = credits.artist.clone();
    candidate.title = credits.title.clone();
    candidate.artist_credits = credits.artists;
    if let Some(album_artist) = candidate.album_artist.as_deref() {
        let album_credits = crate::domain::credits::normalize_structured(
            album_artist,
            "",
            std::mem::take(&mut candidate.album_artist_credits),
        );
        candidate.album_artist = Some(album_credits.artist);
        candidate.album_artist_credits = album_credits.artists;
    }
    set(tag, ItemKey::TrackTitle, &credits.title);
    set(tag, ItemKey::TrackArtist, &credits.artist);
    push_text_values(
        tag,
        ItemKey::TrackArtists,
        crate::domain::credits::individual_names(&candidate.artist_credits),
    );
    optional(tag, ItemKey::AlbumTitle, &candidate.album);
    optional(tag, ItemKey::AlbumArtist, &candidate.album_artist);
    push_text_values(
        tag,
        ItemKey::AlbumArtists,
        crate::domain::credits::individual_names(&candidate.album_artist_credits),
    );
    optional(tag, ItemKey::Isrc, &candidate.isrc);
    optional(tag, ItemKey::Genre, &candidate.genre);
    if let Some(language_code) = candidate
        .score_breakdown
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|value| value["genre"]["language_code"].as_str().map(str::to_owned))
    {
        set(tag, ItemKey::Language, &language_code);
    }
    optional(tag, ItemKey::Composer, &candidate.composer);
    optional(tag, ItemKey::Label, &candidate.label);
    // Navidrome: DATE is the recording date, RELEASEDATE the release (it groups and sorts
    // albums by it) and ORIGINALDATE the first release. The full date when a source gave one.
    optional(tag, ItemKey::RecordingDate, &candidate.year);
    optional(
        tag,
        ItemKey::ReleaseDate,
        &full_date(&candidate.release_date).or_else(|| candidate.year.clone()),
    );
    optional(
        tag,
        ItemKey::OriginalReleaseDate,
        &full_date(&candidate.original_date),
    );
    // Latin sort names keep Persian-named artists in alphabetical order.
    optional(tag, ItemKey::TrackArtistSortOrder, &candidate.artist_sort);
    optional(
        tag,
        ItemKey::AlbumArtistSortOrder,
        &candidate.album_artist_sort,
    );
    optional(
        tag,
        ItemKey::MusicBrainzRecordingId,
        &musicbrainz_id(&candidate.recording_id),
    );
    optional(
        tag,
        ItemKey::MusicBrainzReleaseId,
        &musicbrainz_id(&candidate.release_id),
    );
    optional(
        tag,
        ItemKey::MusicBrainzReleaseGroupId,
        &musicbrainz_id(&candidate.release_group_id),
    );
    optional(
        tag,
        ItemKey::MusicBrainzTrackId,
        &musicbrainz_id(&candidate.release_track_id),
    );
    let artist_ids = candidate
        .artist_credits
        .iter()
        .filter_map(|credit| credit.musicbrainz_id.clone())
        .chain(candidate.artist_id.clone())
        .collect::<Vec<_>>();
    push_text_values(tag, ItemKey::MusicBrainzArtistId, artist_ids);
    let album_artist_ids = candidate
        .album_artist_credits
        .iter()
        .filter_map(|credit| credit.musicbrainz_id.clone())
        .chain(candidate.album_artist_id.clone())
        .collect::<Vec<_>>();
    push_text_values(tag, ItemKey::MusicBrainzReleaseArtistId, album_artist_ids);
    if candidate.is_compilation {
        set(tag, ItemKey::FlagCompilation, "1");
    }
    // RELEASETYPE (FLAC/Opus) or "MusicBrainz Album Type" (MP3/M4A): lets
    // Navidrome show albums, singles, EPs and compilations apart.
    if let Some(kind) = release_type_tag(&candidate) {
        set(tag, ItemKey::MusicBrainzReleaseType, kind);
    }
    if let Some(v) = candidate.track_number {
        tag.set_track(v as u32);
    }
    if let Some(v) = candidate.track_total {
        tag.set_track_total(v as u32);
    }
    if let Some(v) = candidate.disc_number {
        tag.set_disk(v as u32);
    }
    if let Some(v) = candidate.disc_total {
        tag.set_disk_total(v as u32);
    }
    if let Some(data) = artwork.or(preserved_artwork) {
        let mut picture = picture_from_bytes(data)?;
        picture.set_pic_type(PictureType::CoverFront);
        tag.push_picture(picture);
    }
    if let Some(text) = lyrics.map(str::to_owned).or(preserved_lyrics) {
        set(tag, lyrics_key(tag.tag_type()), &text);
    }
    file.save_to_path(path, WriteOptions::default())?;
    if file.file_type() == lofty::file::FileType::Mp4 {
        merge_duplicate_atoms(path)?;
    }
    Ok(())
}

/// A date as Navidrome reads it: YYYY, YYYY-MM or YYYY-MM-DD; anything else is dropped.
fn full_date(value: &Option<String>) -> Option<String> {
    let value = value.as_deref()?.trim();
    let digits = value.replace('-', "");
    let shape_ok = matches!(value.len(), 4 | 7 | 10)
        && digits.chars().all(|character| character.is_ascii_digit())
        && value.get(4..5).is_none_or(|separator| separator == "-")
        && value.get(7..8).is_none_or(|separator| separator == "-");
    (shape_ok && !value.starts_with("0000")).then(|| value.to_owned())
}

/// Lofty writes one MP4 atom per value of a multi-valued tag (ARTISTS, ALBUMARTISTS, the
/// MusicBrainz artist IDs). TagLib, which Navidrome reads with, keeps only the first of
/// duplicated atoms, so the values are folded into one atom with several data entries.
fn merge_duplicate_atoms(path: &Path) -> Result<()> {
    use lofty::config::ParseOptions;
    use lofty::file::AudioFile;
    use lofty::mp4::{Ilst, Mp4File};
    let mut file = std::fs::File::open(path)?;
    let mut mp4 = Mp4File::read_from(&mut file, ParseOptions::new())?;
    let Some(ilst) = mp4.ilst() else {
        return Ok(());
    };
    let idents = ilst
        .into_iter()
        .map(|atom| atom.ident().clone().into_owned())
        .collect::<Vec<_>>();
    let has_duplicates = idents
        .iter()
        .enumerate()
        .any(|(index, ident)| idents[..index].contains(ident));
    if !has_duplicates {
        return Ok(());
    }
    let mut merged = Ilst::new();
    for atom in ilst.clone() {
        // `insert` joins an atom with the same identifier into one with several values.
        merged.insert(atom);
    }
    mp4.set_ilst(merged);
    mp4.save_to_path(path, WriteOptions::default())?;
    Ok(())
}

/// ID3v2 (MP3) only has unsynchronised lyrics (USLT); the LRC text with its
/// timestamps goes there. Vorbis comments (FLAC, Opus, Ogg) use `LYRICS` and
/// MP4 uses `©lyr`. Navidrome reads LRC timestamps in all of them as synced.
fn lyrics_key(tag_type: TagType) -> ItemKey {
    if tag_type == TagType::Id3v2 {
        ItemKey::UnsyncLyrics
    } else {
        ItemKey::Lyrics
    }
}

fn embedded_lyrics(file: &lofty::file::TaggedFile) -> Option<String> {
    file.tags().iter().find_map(|tag| {
        [ItemKey::Lyrics, ItemKey::UnsyncLyrics]
            .into_iter()
            .find_map(|key| tag.get_string(key))
            .filter(|text| !text.trim().is_empty())
            .map(str::to_owned)
    })
}

/// The tags of a library file exactly as written, for the Files tab. Unlike
/// `domain::audio::read`, nothing is cleaned up or guessed from the file name.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct SongTags {
    pub title: Option<String>,
    pub artist: Option<String>,
    /// The separate ARTISTS values (empty when the file has none).
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track: Option<u32>,
    pub track_total: Option<u32>,
    pub disc: Option<u32>,
    pub disc_total: Option<u32>,
    pub year: Option<String>,
    pub release_date: Option<String>,
    pub genre: Option<String>,
    pub composer: Option<String>,
    pub label: Option<String>,
    pub isrc: Option<String>,
    pub musicbrainz_recording: Option<String>,
    pub musicbrainz_release: Option<String>,
    pub track_gain: Option<String>,
    pub album_gain: Option<String>,
    pub lyrics: Option<String>,
    pub has_cover: bool,
    pub duration: f64,
    pub bitrate: Option<u32>,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u8>,
    pub channels: Option<u8>,
}

pub fn read_details(path: &Path) -> Result<SongTags> {
    let file = Probe::open(path)?.guess_file_type()?.read()?;
    let props = file.properties();
    let mut details = SongTags {
        duration: props.duration().as_secs_f64(),
        bitrate: props.audio_bitrate(),
        sample_rate: props.sample_rate(),
        bit_depth: props.bit_depth(),
        channels: props.channels(),
        lyrics: embedded_lyrics(&file),
        has_cover: valid_embedded_artwork(&file).is_some(),
        ..SongTags::default()
    };
    let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) else {
        return Ok(details);
    };
    let text = |key: ItemKey| {
        tag.get_string(key)
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    };
    details.title = text(ItemKey::TrackTitle);
    details.artist = text(ItemKey::TrackArtist);
    // MP4 keeps all ARTISTS values in one atom, of which the generic tag shows the first.
    let artists: Vec<String> = match file.file_type() {
        lofty::file::FileType::Mp4 => read_ilst(path)?
            .map(|ilst| mp4_values(&ilst, ItemKey::TrackArtists))
            .unwrap_or_default(),
        _ => tag
            .get_strings(ItemKey::TrackArtists)
            .map(str::to_owned)
            .collect(),
    };
    details.artists = artists
        .into_iter()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .collect();
    details.album = text(ItemKey::AlbumTitle);
    details.album_artist = text(ItemKey::AlbumArtist);
    details.track = tag.track();
    details.track_total = tag.track_total();
    details.disc = tag.disk();
    details.disc_total = tag.disk_total();
    details.year = text(ItemKey::RecordingDate).or_else(|| text(ItemKey::Year));
    details.release_date = text(ItemKey::ReleaseDate);
    details.genre = text(ItemKey::Genre);
    details.composer = text(ItemKey::Composer);
    details.label = text(ItemKey::Label);
    details.isrc = text(ItemKey::Isrc);
    details.musicbrainz_recording = text(ItemKey::MusicBrainzRecordingId);
    details.musicbrainz_release = text(ItemKey::MusicBrainzReleaseId);
    details.track_gain = text(ItemKey::ReplayGainTrackGain);
    details.album_gain = text(ItemKey::ReplayGainAlbumGain);
    Ok(details)
}

/// Fields changed by hand on the Files tab. `None` leaves a field alone,
/// `Some(None)` removes it, `Some(Some(value))` writes it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TagEdit {
    pub title: Option<Option<String>>,
    pub artist: Option<Option<String>>,
    pub album: Option<Option<String>>,
    pub album_artist: Option<Option<String>>,
    pub track: Option<Option<u32>>,
    pub track_total: Option<Option<u32>>,
    pub disc: Option<Option<u32>>,
    pub disc_total: Option<Option<u32>>,
    pub year: Option<Option<String>>,
    pub genre: Option<Option<String>>,
    pub composer: Option<Option<String>>,
    pub label: Option<Option<String>>,
    pub isrc: Option<Option<String>>,
}

impl TagEdit {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Writes only the fields in `edit`, leaving every other tag, the cover and
/// the lyrics alone. A new artist also rewrites the separate ARTISTS values
/// Navidrome credits ("A feat. B" gives A and B; "A & B" stays one name) and
/// drops that artist's sort name and MusicBrainz IDs, which belonged to the
/// old name. A new year also moves a release date from another year.
pub fn edit_fields(path: &Path, edit: &TagEdit) -> Result<()> {
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if matches!(extension.as_str(), "wav" | "aiff" | "aif") {
        bail!("Tags of WAV and AIFF files are not edited here");
    }
    let mut file = Probe::open(path)?.guess_file_type()?.read()?;
    if file.primary_tag().is_none() {
        file.insert_tag(Tag::new(file.primary_tag_type()));
    }
    let tag = file.primary_tag_mut().expect("primary tag present");
    let text = |tag: &mut Tag, key: ItemKey, value: &Option<Option<String>>| match value {
        None => {}
        Some(None) => tag.remove_key(key),
        Some(Some(value)) => set(tag, key, value),
    };
    text(tag, ItemKey::TrackTitle, &edit.title);
    text(tag, ItemKey::AlbumTitle, &edit.album);
    text(tag, ItemKey::Genre, &edit.genre);
    text(tag, ItemKey::Composer, &edit.composer);
    text(tag, ItemKey::Label, &edit.label);
    text(tag, ItemKey::Isrc, &edit.isrc);
    for (value, display, list, sort, ids) in [
        (
            &edit.artist,
            ItemKey::TrackArtist,
            ItemKey::TrackArtists,
            ItemKey::TrackArtistSortOrder,
            ItemKey::MusicBrainzArtistId,
        ),
        (
            &edit.album_artist,
            ItemKey::AlbumArtist,
            ItemKey::AlbumArtists,
            ItemKey::AlbumArtistSortOrder,
            ItemKey::MusicBrainzReleaseArtistId,
        ),
    ] {
        let Some(value) = value else {
            continue;
        };
        for key in [display, list, sort, ids] {
            tag.remove_key(key);
        }
        if let Some(name) = value {
            set(tag, display, name);
            let credits = crate::domain::credits::normalize_featured(name, "");
            push_text_values(
                tag,
                list,
                crate::domain::credits::individual_names(&credits.artists),
            );
        }
    }
    match &edit.year {
        None => {}
        Some(None) => {
            for key in [ItemKey::RecordingDate, ItemKey::Year, ItemKey::ReleaseDate] {
                tag.remove_key(key);
            }
        }
        Some(Some(year)) => {
            tag.remove_key(ItemKey::Year);
            set(tag, ItemKey::RecordingDate, year);
            if tag
                .get_string(ItemKey::ReleaseDate)
                .is_some_and(|date| !date.trim().starts_with(year.as_str()))
            {
                set(tag, ItemKey::ReleaseDate, year);
            }
        }
    }
    match edit.track {
        None => {}
        Some(None) => tag.remove_track(),
        Some(Some(number)) => tag.set_track(number),
    }
    match edit.track_total {
        None => {}
        Some(None) => tag.remove_track_total(),
        Some(Some(number)) => tag.set_track_total(number),
    }
    match edit.disc {
        None => {}
        Some(None) => tag.remove_disk(),
        Some(Some(number)) => tag.set_disk(number),
    }
    match edit.disc_total {
        None => {}
        Some(None) => tag.remove_disk_total(),
        Some(Some(number)) => tag.set_disk_total(number),
    }
    file.save_to_path(path, WriteOptions::default())?;
    if file.file_type() == lofty::file::FileType::Mp4 {
        merge_duplicate_atoms(path)?;
    }
    Ok(())
}

pub fn read_lyrics(path: &Path) -> Result<Option<String>> {
    let file = Probe::open(path)?.guess_file_type()?.read()?;
    Ok(embedded_lyrics(&file))
}

/// Adds lyrics to a filed song without touching its other tags.
pub fn embed_lyrics(path: &Path, text: &str) -> Result<()> {
    let mut file = Probe::open(path)?.guess_file_type()?.read()?;
    if file.primary_tag().is_none() {
        file.insert_tag(Tag::new(file.primary_tag_type()));
    }
    let tag = file.primary_tag_mut().expect("primary tag present");
    set(tag, lyrics_key(tag.tag_type()), text);
    file.save_to_path(path, WriteOptions::default())?;
    verify_lyrics(path, text)
}

/// Replaces one artist with `new` (one or more artists) in the artist and
/// album-artist tags of a filed song, leaving every other tag alone. Returns
/// whether anything changed. Used when the owner renames, merges or splits an
/// artist.
pub fn replace_artist(path: &Path, old: &str, new: &[String]) -> Result<bool> {
    use crate::domain::names::match_key;
    let mut file = Probe::open(path)?.guess_file_type()?.read()?;
    let Some(tag) = file.primary_tag_mut() else {
        return Ok(false);
    };
    let old_key = match_key(old);
    let joined = new.join(" & ");
    let mut changed = false;
    for (display_key, list_key) in [
        (ItemKey::TrackArtist, ItemKey::TrackArtists),
        (ItemKey::AlbumArtist, ItemKey::AlbumArtists),
    ] {
        let values = tag
            .get_strings(list_key)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let display = tag.get_string(display_key).map(str::to_owned);
        let names_old = values.iter().any(|value| match_key(value) == old_key)
            || (values.is_empty()
                && display
                    .as_deref()
                    .is_some_and(|value| match_key(value) == old_key));
        if names_old {
            let mut replaced = Vec::new();
            for value in if values.is_empty() {
                vec![old.to_owned()]
            } else {
                values
            } {
                if match_key(&value) == old_key {
                    replaced.extend(new.iter().cloned());
                } else {
                    replaced.push(value);
                }
            }
            tag.remove_key(list_key);
            push_text_values(tag, list_key, replaced);
            changed = true;
        }
        if let Some(display) = display {
            let updated = if match_key(&display) == old_key {
                joined.clone()
            } else if display.contains(old) {
                display.replace(old, &joined)
            } else {
                display.clone()
            };
            if updated != display {
                set(tag, display_key, &updated);
                changed = true;
            }
        }
    }
    if changed {
        file.save_to_path(path, WriteOptions::default())?;
    }
    Ok(changed)
}

/// What makes songs one album in Navidrome: it groups by album name, album artist (or its
/// MusicBrainz ID), MusicBrainz album ID and release date. Songs of one album must agree on
/// all of them, or Navidrome shows the album twice.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AlbumTags {
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub album_artists: Vec<String>,
    pub album_artist_sort: Option<String>,
    pub release_artist_ids: Vec<String>,
    /// RELEASEDATE: Navidrome groups albums by it.
    pub release_date: Option<String>,
    pub original_date: Option<String>,
    /// DATE.
    pub year: Option<String>,
    pub release_id: Option<String>,
    pub release_group_id: Option<String>,
    pub release_type: Option<String>,
    pub compilation: bool,
    pub label: Option<String>,
}

impl AlbumTags {
    /// The values Navidrome tells albums apart by. Two songs with different keys are two
    /// albums there, whatever their folder.
    pub fn grouping_key(&self) -> (String, String, String, String) {
        let key = |value: &Option<String>| {
            value
                .as_deref()
                .map(crate::domain::names::match_key)
                .unwrap_or_default()
        };
        let artist = if self.release_artist_ids.is_empty() {
            key(&self.album_artist)
        } else {
            self.release_artist_ids.join("|").to_ascii_lowercase()
        };
        (
            key(&self.album),
            artist,
            self.release_date.clone().unwrap_or_default(),
            self.release_id
                .clone()
                .unwrap_or_default()
                .to_ascii_lowercase(),
        )
    }
}

const ALBUM_TEXT_KEYS: [ItemKey; 11] = [
    ItemKey::AlbumTitle,
    ItemKey::AlbumArtist,
    ItemKey::AlbumArtistSortOrder,
    ItemKey::ReleaseDate,
    ItemKey::OriginalReleaseDate,
    ItemKey::RecordingDate,
    ItemKey::MusicBrainzReleaseId,
    ItemKey::MusicBrainzReleaseGroupId,
    ItemKey::MusicBrainzReleaseType,
    ItemKey::FlagCompilation,
    ItemKey::Label,
];

/// The album details of a song (see [`AlbumTags`]).
pub fn read_album_tags(path: &Path) -> Result<AlbumTags> {
    let file = Probe::open(path)?.guess_file_type()?.read()?;
    let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) else {
        return Ok(AlbumTags::default());
    };
    let text = |key: ItemKey| {
        tag.get_string(key)
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    };
    let list = |key: ItemKey| {
        tag.get_strings(key)
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
    };
    Ok(AlbumTags {
        album: text(ItemKey::AlbumTitle),
        album_artist: text(ItemKey::AlbumArtist),
        album_artists: list(ItemKey::AlbumArtists),
        album_artist_sort: text(ItemKey::AlbumArtistSortOrder),
        release_artist_ids: list(ItemKey::MusicBrainzReleaseArtistId),
        release_date: text(ItemKey::ReleaseDate),
        original_date: text(ItemKey::OriginalReleaseDate),
        year: text(ItemKey::RecordingDate).or_else(|| text(ItemKey::Year)),
        release_id: text(ItemKey::MusicBrainzReleaseId),
        release_group_id: text(ItemKey::MusicBrainzReleaseGroupId),
        release_type: text(ItemKey::MusicBrainzReleaseType),
        compilation: text(ItemKey::FlagCompilation).is_some_and(|value| value == "1"),
        label: text(ItemKey::Label),
    })
}

/// Gives a song exactly these album details: every one is written, and every one the album
/// lacks is removed, so the song and the album agree. Returns whether anything changed.
pub fn write_album_tags(path: &Path, album: &AlbumTags) -> Result<bool> {
    if read_album_tags(path)? == *album {
        return Ok(false);
    }
    let mut file = Probe::open(path)?.guess_file_type()?.read()?;
    if file.primary_tag().is_none() {
        let kind = file.primary_tag_type();
        file.insert_tag(Tag::new(kind));
    }
    let tag = file
        .primary_tag_mut()
        .context("the song has no tag to write")?;
    for key in ALBUM_TEXT_KEYS.into_iter().chain([
        ItemKey::Year,
        ItemKey::AlbumArtists,
        ItemKey::MusicBrainzReleaseArtistId,
    ]) {
        tag.remove_key(key);
    }
    optional(tag, ItemKey::AlbumTitle, &album.album);
    optional(tag, ItemKey::AlbumArtist, &album.album_artist);
    push_text_values(tag, ItemKey::AlbumArtists, album.album_artists.clone());
    optional(tag, ItemKey::AlbumArtistSortOrder, &album.album_artist_sort);
    push_text_values(
        tag,
        ItemKey::MusicBrainzReleaseArtistId,
        album.release_artist_ids.clone(),
    );
    optional(tag, ItemKey::ReleaseDate, &album.release_date);
    optional(tag, ItemKey::OriginalReleaseDate, &album.original_date);
    optional(tag, ItemKey::RecordingDate, &album.year);
    optional(tag, ItemKey::MusicBrainzReleaseId, &album.release_id);
    optional(
        tag,
        ItemKey::MusicBrainzReleaseGroupId,
        &album.release_group_id,
    );
    optional(tag, ItemKey::MusicBrainzReleaseType, &album.release_type);
    if album.compilation {
        set(tag, ItemKey::FlagCompilation, "1");
    }
    optional(tag, ItemKey::Label, &album.label);
    file.save_to_path(path, WriteOptions::default())?;
    Ok(true)
}

/// Writes the title and performer Telegram shows for a sent file into it.
/// Files forwarded from music channels often have no tags or a channel's name
/// as artist, while the message carries the right ones (for your own files
/// they are the same as the tags). Promotional values are ignored. Returns
/// whether anything was written.
pub fn apply_telegram_details(
    path: &Path,
    title: Option<&str>,
    artist: Option<&str>,
) -> Result<bool> {
    let usable = |value: Option<&str>| {
        value
            .map(str::trim)
            .filter(|value| !value.is_empty() && !crate::domain::audio::is_promotional(value))
            .map(str::to_owned)
    };
    let (title, artist) = (usable(title), usable(artist));
    if title.is_none() && artist.is_none() {
        return Ok(false);
    }
    let mut file = Probe::open(path)?.guess_file_type()?.read()?;
    if file.primary_tag().is_none() {
        file.insert_tag(Tag::new(file.primary_tag_type()));
    }
    let tag = file.primary_tag_mut().expect("primary tag present");
    let mut changed = false;
    if let Some(title) = title
        && tag.title().as_deref() != Some(title.as_str())
    {
        tag.set_title(title);
        changed = true;
    }
    if let Some(artist) = artist
        && tag.artist().as_deref() != Some(artist.as_str())
    {
        tag.set_artist(artist);
        changed = true;
    }
    if changed {
        file.save_to_path(path, WriteOptions::default())?;
    }
    Ok(changed)
}

pub fn verify_lyrics(path: &Path, expected: &str) -> Result<()> {
    let written = read_lyrics(path)?.unwrap_or_default();
    if written.trim() != expected.trim() {
        bail!("written lyrics do not match");
    }
    Ok(())
}

pub fn verify_written_metadata(path: &Path, candidate: &Candidate) -> Result<()> {
    let file = Probe::open(path)?.guess_file_type()?.read()?;
    let tag = file
        .primary_tag()
        .or_else(|| file.first_tag())
        .ok_or_else(|| anyhow::anyhow!("written file contains no metadata tag"))?;
    // Lofty reads only the first value of an MP4 atom that holds several (TagLib reads
    // them all), so multi-values of an M4A are checked on the atoms themselves.
    let ilst = (file.file_type() == lofty::file::FileType::Mp4)
        .then(|| read_ilst(path))
        .transpose()?
        .flatten();
    let credits = crate::domain::credits::normalize_structured(
        &candidate.artist,
        &candidate.title,
        candidate.artist_credits.clone(),
    );
    if tag.title().as_deref() != Some(credits.title.as_str()) {
        bail!("written title does not match the canonical title");
    }
    if tag.artist().as_deref() != Some(credits.artist.as_str()) {
        bail!("written display artist does not match the canonical credit");
    }
    verify_multi_values(
        tag,
        ilst.as_ref(),
        ItemKey::TrackArtists,
        &crate::domain::credits::individual_names(&credits.artists),
        "track artists",
    )?;
    if let Some(album_artist) = candidate.album_artist.as_deref()
        && tag.get_string(ItemKey::AlbumArtist) != Some(album_artist)
    {
        bail!("written display album artist does not match the canonical credit");
    }
    if !candidate.album_artist_credits.is_empty() {
        verify_multi_values(
            tag,
            ilst.as_ref(),
            ItemKey::AlbumArtists,
            &crate::domain::credits::individual_names(&candidate.album_artist_credits),
            "album artists",
        )?;
    }
    let artist_ids = candidate
        .artist_credits
        .iter()
        .filter_map(|credit| credit.musicbrainz_id.clone())
        .chain(candidate.artist_id.clone())
        .collect::<Vec<_>>();
    if !artist_ids.is_empty() {
        verify_multi_values(
            tag,
            ilst.as_ref(),
            ItemKey::MusicBrainzArtistId,
            &dedupe_values(artist_ids),
            "MusicBrainz artist IDs",
        )?;
    }
    let album_artist_ids = candidate
        .album_artist_credits
        .iter()
        .filter_map(|credit| credit.musicbrainz_id.clone())
        .chain(candidate.album_artist_id.clone())
        .collect::<Vec<_>>();
    if !album_artist_ids.is_empty() {
        verify_multi_values(
            tag,
            ilst.as_ref(),
            ItemKey::MusicBrainzReleaseArtistId,
            &dedupe_values(album_artist_ids),
            "MusicBrainz album artist IDs",
        )?;
    }
    let written_compilation = tag.get_string(ItemKey::FlagCompilation) == Some("1");
    if written_compilation != candidate.is_compilation {
        bail!("written compilation flag differs from the selected metadata");
    }
    Ok(())
}

fn verify_multi_values(
    tag: &Tag,
    ilst: Option<&lofty::mp4::Ilst>,
    key: ItemKey,
    expected: &[String],
    label: &str,
) -> Result<()> {
    let actual = match ilst {
        Some(ilst) => mp4_values(ilst, key),
        None => tag.get_strings(key).map(str::to_owned).collect::<Vec<_>>(),
    };
    if actual != expected {
        bail!("written {label} differ: expected {expected:?}, found {actual:?}");
    }
    Ok(())
}

fn read_ilst(path: &Path) -> Result<Option<lofty::mp4::Ilst>> {
    use lofty::config::ParseOptions;
    use lofty::file::AudioFile;
    let mut file = std::fs::File::open(path)?;
    let mp4 = lofty::mp4::Mp4File::read_from(&mut file, ParseOptions::new())?;
    Ok(mp4.ilst().cloned())
}

/// Every value of the iTunes free-form atom that `key` is written to.
fn mp4_values(ilst: &lofty::mp4::Ilst, key: ItemKey) -> Vec<String> {
    let name = match key {
        ItemKey::TrackArtists => "ARTISTS",
        ItemKey::AlbumArtists => "ALBUMARTISTS",
        ItemKey::MusicBrainzArtistId => "MusicBrainz Artist Id",
        ItemKey::MusicBrainzReleaseArtistId => "MusicBrainz Album Artist Id",
        _ => return Vec::new(),
    };
    ilst.into_iter()
        .filter(|atom| {
            matches!(atom.ident(), lofty::mp4::AtomIdent::Freeform { mean, name: atom_name }
                if mean == "com.apple.iTunes" && atom_name == name)
        })
        .flat_map(|atom| atom.data())
        .filter_map(|data| match data {
            lofty::mp4::AtomData::UTF8(text) | lofty::mp4::AtomData::UTF16(text) => {
                Some(text.clone())
            }
            _ => None,
        })
        .collect()
}

fn dedupe_values(values: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for value in values {
        if !out
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(&value))
        {
            out.push(value);
        }
    }
    out
}

fn push_text_values(tag: &mut Tag, key: ItemKey, values: Vec<String>) {
    for value in dedupe_values(values) {
        let _ = tag.push(TagItem::new(key, ItemValue::Text(value)));
    }
}

pub fn write_resilient(
    path: &Path,
    candidate: &Candidate,
    artwork: Option<Vec<u8>>,
    lyrics: Option<&str>,
) -> Result<bool> {
    match write(path, candidate, artwork.clone(), lyrics) {
        Ok(()) => Ok(false),
        Err(initial_error) => {
            sanitize_legacy_metadata(path).with_context(|| {
                format!("initial tag read failed ({initial_error}); lossless tag cleanup failed")
            })?;
            write(path, candidate, artwork, lyrics).with_context(|| {
                format!("tag cleanup succeeded, but writing still failed ({initial_error})")
            })?;
            Ok(true)
        }
    }
}

fn sanitize_legacy_metadata(path: &Path) -> Result<()> {
    let extension = detected_container_extension(path).unwrap_or_else(|| {
        path.extension()
            .and_then(|value| value.to_str())
            .unwrap_or("audio")
            .to_ascii_lowercase()
    });
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("audio");
    let repaired = path.with_file_name(format!(
        ".{stem}.tag-clean-{}.{}",
        std::process::id(),
        extension
    ));
    let _ = std::fs::remove_file(&repaired);
    let output = std::process::Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-fflags",
            "+discardcorrupt",
            "-i",
        ])
        .arg(path)
        .args([
            "-map",
            "0:a:0",
            "-c:a",
            "copy",
            "-map_metadata",
            "-1",
            "-map_chapters",
            "-1",
            "-y",
        ])
        .arg(&repaired)
        .output()
        .context("could not start FFmpeg for lossless tag cleanup")?;
    if !output.status.success() || !repaired.is_file() {
        let _ = std::fs::remove_file(&repaired);
        bail!(
            "FFmpeg could not remove malformed metadata: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    std::fs::rename(&repaired, path).with_context(|| {
        format!(
            "could not replace temporary copy with sanitized audio {}",
            path.display()
        )
    })?;
    Ok(())
}

fn detected_container_extension(path: &Path) -> Option<String> {
    let output = std::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=format_name",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path)
        .output()
        .ok()?;
    let format = String::from_utf8_lossy(&output.stdout);
    let names = format.trim().split(',').collect::<Vec<_>>();
    if names
        .iter()
        .any(|name| matches!(*name, "mov" | "mp4" | "m4a" | "3gp" | "3g2" | "mj2"))
    {
        Some("m4a".into())
    } else if names.contains(&"mp3") {
        Some("mp3".into())
    } else if names.contains(&"flac") {
        Some("flac".into())
    } else if names.contains(&"ogg") {
        Some("ogg".into())
    } else {
        None
    }
}

fn valid_embedded_artwork(file: &lofty::file::TaggedFile) -> Option<Vec<u8>> {
    let valid_picture = |tag: &Tag| {
        tag.pictures()
            .iter()
            .find(|picture| {
                matches!(
                    picture.pic_type(),
                    PictureType::CoverFront | PictureType::Other
                ) && validate_artwork(picture.data()).is_ok()
            })
            .map(|picture| picture.data().to_vec())
    };
    file.primary_tag().and_then(valid_picture).or_else(|| {
        file.tags()
            .iter()
            .filter(|tag| Some(tag.tag_type()) != file.primary_tag().map(Tag::tag_type))
            .find_map(valid_picture)
    })
}

pub fn read_artwork(path: &Path) -> Result<Option<Vec<u8>>> {
    let file = Probe::open(path)?.guess_file_type()?.read()?;
    Ok(valid_embedded_artwork(&file))
}

/// Verify the on-disk representation players actually consume. FLAC artwork is
/// required to be a native PICTURE metadata block; reading it back through the
/// same generic tag abstraction could otherwise hide a format-specific write bug.
pub fn verify_embedded_artwork(path: &Path, expected: &[u8]) -> Result<()> {
    let file = Probe::open(path)?.guess_file_type()?.read()?;
    if file.file_type() == lofty::file::FileType::Flac {
        let embedded = native_flac_front_cover(path)?
            .ok_or_else(|| anyhow::anyhow!("FLAC contains no native front-cover PICTURE block"))?;
        if embedded != expected {
            bail!("native FLAC cover does not match the validated preview image");
        }
        return Ok(());
    }
    let embedded = valid_embedded_artwork(&file)
        .ok_or_else(|| anyhow::anyhow!("cover verification found no embedded image"))?;
    if embedded != expected {
        bail!("embedded cover does not match the validated preview image");
    }
    Ok(())
}

/// Only real MusicBrainz IDs go into MusicBrainz tags.
fn musicbrainz_id(value: &Option<String>) -> Option<String> {
    value.clone().filter(|id| is_mbid(id))
}

/// The front cover from the FLAC stream's own PICTURE metadata blocks (not
/// a picture inside the Vorbis comments).
fn native_flac_front_cover(path: &Path) -> Result<Option<Vec<u8>>> {
    let mut file = std::fs::File::open(path)?;
    let flac = FlacFile::read_from(&mut file, ParseOptions::new())?;
    Ok(flac
        .pictures()
        .iter()
        .find(|(picture, _)| picture.pic_type() == PictureType::CoverFront)
        .map(|(picture, _)| picture.data().to_vec()))
}

fn picture_from_bytes(data: Vec<u8>) -> Result<Picture> {
    let mut reader = std::io::Cursor::new(data);
    Ok(Picture::from_reader(&mut reader)?)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtworkInfo {
    pub width: u32,
    pub height: u32,
    pub mime: &'static str,
    pub sha256: String,
}

/// What [`check_artwork`] learned about an acceptable image.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtworkCheck {
    pub info: ArtworkInfo,
    /// One plain colour: usually a placeholder, unless the release is known.
    pub single_colour: bool,
}

/// Checks that `data` is a usable cover: a supported format, square-ish, at
/// least 300×300 and not blank. The size is read from the header before the
/// image is decoded, so huge images are refused cheaply.
pub fn check_artwork(data: &[u8]) -> Result<ArtworkCheck> {
    if data.len() > MAX_ARTWORK_BYTES {
        bail!("cover image exceeds the 20 MB safety limit");
    }
    let format = image::guess_format(data).context("cover image format is unsupported")?;
    let mime = match format {
        image::ImageFormat::Jpeg => "image/jpeg",
        image::ImageFormat::Png => "image/png",
        image::ImageFormat::Gif => "image/gif",
        image::ImageFormat::WebP => "image/webp",
        _ => bail!("cover image format is unsupported"),
    };
    let (width, height) = image::ImageReader::with_format(std::io::Cursor::new(data), format)
        .into_dimensions()
        .context("cover image could not be read")?;
    if width < 300 || height < 300 {
        bail!("cover image is too small ({width}x{height}; minimum is 300x300)");
    }
    if width > 12_000 || height > 12_000 || u64::from(width) * u64::from(height) > 100_000_000 {
        bail!("cover image decoded dimensions exceed the safety limit");
    }
    let ratio = width as f64 / height as f64;
    if !(0.75..=1.333_334).contains(&ratio) {
        bail!("cover image aspect ratio is not credible album artwork");
    }
    let decoded = image::load_from_memory_with_format(data, format)
        .context("cover image could not be fully decoded")?;
    let rgba = decoded.to_rgba8();
    let step = ((rgba.len() / 4) / 100_000).max(1);
    let mut visible = 0_usize;
    let mut sampled = 0_usize;
    let mut min = [u8::MAX; 3];
    let mut max = [u8::MIN; 3];
    for pixel in rgba.pixels().step_by(step) {
        sampled += 1;
        if pixel[3] <= 16 {
            continue;
        }
        visible += 1;
        for channel in 0..3 {
            min[channel] = min[channel].min(pixel[channel]);
            max[channel] = max[channel].max(pixel[channel]);
        }
    }
    if sampled == 0 || visible * 50 < sampled {
        bail!("cover image is blank or almost fully transparent");
    }
    let single_colour = (0..3).all(|channel| max[channel].saturating_sub(min[channel]) <= 1);
    // Lofty must also recognize the bytes because it is responsible for the
    // actual container-specific embedding operation.
    Picture::from_reader(&mut std::io::Cursor::new(data))?;
    Ok(ArtworkCheck {
        info: ArtworkInfo {
            width,
            height,
            mime,
            sha256: hex::encode(Sha256::digest(data)),
        },
        single_colour,
    })
}

/// The longest side of a cover inside a song; catalogs send up to 3000 px.
pub const EMBEDDED_COVER_SIZE: u32 = 1200;

/// The cover as it goes into the file and next to it: JPEG, at most 1200 px on the long
/// side. A JPEG that already fits is kept byte for byte, so its verified hash stays valid.
pub fn prepare_cover(data: &[u8]) -> Result<Vec<u8>> {
    let format = image::guess_format(data).context("cover image format is unsupported")?;
    let (width, height) = image::ImageReader::with_format(std::io::Cursor::new(data), format)
        .into_dimensions()
        .context("cover image could not be read")?;
    let fits = width.max(height) <= EMBEDDED_COVER_SIZE;
    if fits && format == image::ImageFormat::Jpeg {
        return Ok(data.to_vec());
    }
    let decoded = image::load_from_memory_with_format(data, format)
        .context("cover image could not be decoded")?;
    let resized = if fits {
        decoded
    } else {
        decoded.resize(
            EMBEDDED_COVER_SIZE,
            EMBEDDED_COVER_SIZE,
            image::imageops::FilterType::Lanczos3,
        )
    };
    let mut output = std::io::Cursor::new(Vec::new());
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut output, 88);
    encoder
        .encode_image(&resized.to_rgb8())
        .context("cover image could not be encoded")?;
    Ok(output.into_inner())
}

pub fn inspect_artwork(data: &[u8], trusted_exact_release: bool) -> Result<ArtworkInfo> {
    let check = check_artwork(data)?;
    if check.single_colour && !trusted_exact_release {
        bail!("cover image is a blank constant-color placeholder");
    }
    Ok(check.info)
}

pub fn validate_artwork(data: &[u8]) -> Result<()> {
    inspect_artwork(data, false).map(|_| ())
}

#[cfg(test)]
pub fn test_artwork_png() -> Vec<u8> {
    let image = image::RgbaImage::from_fn(320, 320, |x, y| {
        if (x / 20 + y / 20) % 2 == 0 {
            image::Rgba([32, 96, 180, 255])
        } else {
            image::Rgba([230, 180, 48, 255])
        }
    });
    let mut output = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut output, image::ImageFormat::Png)
        .unwrap();
    output.into_inner()
}
fn set(tag: &mut Tag, key: ItemKey, value: &str) {
    tag.insert_text(key, value.into());
}
/// The MusicBrainz release type in its tag spelling (lowercase).
fn release_type_tag(candidate: &Candidate) -> Option<&'static str> {
    if candidate.is_compilation {
        return Some("compilation");
    }
    match candidate
        .release_type
        .as_deref()
        .map(|kind| kind.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("album") => Some("album"),
        Some("single") => Some("single"),
        Some("ep") => Some("ep"),
        Some("compilation") => Some("compilation"),
        Some("broadcast") => Some("broadcast"),
        _ => None,
    }
}

fn optional(tag: &mut Tag, key: ItemKey, value: &Option<String>) {
    if let Some(value) = value {
        set(tag, key, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lofty::picture::MimeType;

    fn one_pixel_png() -> Vec<u8> {
        test_artwork_png()
    }

    #[test]
    fn cover_image_type_is_detected_before_embedding() {
        let png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0];
        let picture = picture_from_bytes(png).unwrap();
        assert_eq!(picture.mime_type(), Some(&MimeType::Png));
    }

    #[test]
    fn invalid_cover_data_is_rejected() {
        assert!(picture_from_bytes(vec![0; 12]).is_err());
    }

    #[test]
    fn existing_valid_cover_survives_when_catalog_has_no_artwork() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("preserve-cover.mp3");
        let status = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("sine=frequency=440:duration=0.1")
            .args(["-q:a", "9"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        let candidate = Candidate {
            title: "Song".into(),
            artist: "Artist".into(),
            ..Default::default()
        };
        let artwork = one_pixel_png();
        write(&path, &candidate, Some(artwork.clone()), None).unwrap();
        write(&path, &candidate, None, None).unwrap();

        let file = Probe::open(&path).unwrap().read().unwrap();
        assert_eq!(file.primary_tag().unwrap().pictures()[0].data(), artwork);
    }

    #[test]
    fn album_details_copy_from_one_song_to_another() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let tone = |name: &str| {
            let path = directory.path().join(name);
            let status = std::process::Command::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
                .arg("sine=frequency=440:duration=0.1")
                .args(["-q:a", "9"])
                .arg(&path)
                .status()
                .unwrap();
            assert!(status.success());
            path
        };
        let (album_song, odd_song) = (tone("album.mp3"), tone("odd.mp3"));
        let album = Candidate {
            title: "Hamdam".into(),
            artist: "Ebi".into(),
            album: Some("Khalij".into()),
            album_artist: Some("Ebi".into()),
            year: Some("1999".into()),
            release_date: Some("1999-03-01".into()),
            release_type: Some("album".into()),
            ..Default::default()
        };
        write(&album_song, &album, None, None).unwrap();
        let odd = Candidate {
            title: "Khaleej".into(),
            artist: "Ebi".into(),
            album: Some("Khaleej".into()),
            album_artist: Some("Ebi".into()),
            year: Some("1999".into()),
            release_date: Some("1999".into()),
            release_type: Some("single".into()),
            label: Some("Caltex".into()),
            ..Default::default()
        };
        write(&odd_song, &odd, None, None).unwrap();

        let wanted = read_album_tags(&album_song).unwrap();
        assert_eq!(wanted.album.as_deref(), Some("Khalij"));
        assert_eq!(wanted.release_date.as_deref(), Some("1999-03-01"));
        assert_ne!(
            read_album_tags(&odd_song).unwrap().grouping_key(),
            wanted.grouping_key()
        );
        assert!(write_album_tags(&odd_song, &wanted).unwrap());
        let now = read_album_tags(&odd_song).unwrap();
        assert_eq!(now, wanted);
        assert_eq!(now.grouping_key(), wanted.grouping_key());
        // The label only the odd song had is gone: the album has none.
        assert!(now.label.is_none());
        // The song's own details stay.
        let details = read_details(&odd_song).unwrap();
        assert_eq!(details.title.as_deref(), Some("Khaleej"));
        // Already the same: nothing is written.
        assert!(!write_album_tags(&odd_song, &wanted).unwrap());
    }

    #[test]
    fn a_hand_edit_changes_only_its_fields() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        for (name, codec) in [
            ("edit.mp3", vec!["-q:a", "9"]),
            ("edit.flac", vec!["-c:a", "flac"]),
            ("edit.m4a", vec!["-c:a", "aac"]),
        ] {
            let path = directory.path().join(name);
            let status = std::process::Command::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
                .arg("sine=frequency=440:duration=0.3")
                .args(codec)
                .arg(&path)
                .status()
                .unwrap();
            assert!(status.success());
            let filed = Candidate {
                title: "Get Lucky".into(),
                artist: "Daft Punk".into(),
                artist_credits: vec![crate::domain::credits::ArtistCredit {
                    musicbrainz_id: Some("056e4f3e-d505-4dad-8ec1-d04f521cbb56".into()),
                    ..crate::domain::credits::ArtistCredit::new("Daft Punk", "")
                }],
                album: Some("Random Access Memories".into()),
                genre: Some("Electronic".into()),
                composer: Some("Thomas Bangalter".into()),
                isrc: Some("USQX91300108".into()),
                year: Some("2013".into()),
                release_date: Some("2013-05-17".into()),
                recording_id: Some("f7c0d7bf-0b2a-4b6a-8b4b-1d6f0e0c6d11".into()),
                track_number: Some(8),
                track_total: Some(13),
                ..Default::default()
            };
            write(&path, &filed, Some(one_pixel_png()), None).unwrap();
            let before = read_details(&path).unwrap();

            edit_fields(
                &path,
                &TagEdit {
                    artist: Some(Some("Daft Punk feat. Pharrell Williams".into())),
                    genre: Some(Some("Disco".into())),
                    composer: Some(None),
                    year: Some(Some("2014".into())),
                    track_total: Some(Some(14)),
                    ..TagEdit::default()
                },
            )
            .unwrap();

            let details = read_details(&path).unwrap();
            assert_eq!(details.title.as_deref(), Some("Get Lucky"), "{name}");
            assert_eq!(
                details.artist.as_deref(),
                Some("Daft Punk feat. Pharrell Williams"),
                "{name}"
            );
            assert_eq!(
                details.artists,
                ["Daft Punk", "Pharrell Williams"],
                "{name}"
            );
            assert_eq!(details.genre.as_deref(), Some("Disco"), "{name}");
            assert_eq!(details.composer, None, "{name}");
            assert_eq!(details.year.as_deref(), Some("2014"), "{name}");
            assert_eq!(details.release_date.as_deref(), Some("2014"), "{name}");
            assert_eq!(
                (details.track, details.track_total),
                (Some(8), Some(14)),
                "{name}"
            );
            // Untouched tags, the cover and the recording ID stay.
            assert_eq!(details.isrc.as_deref(), Some("USQX91300108"), "{name}");
            assert_eq!(
                details.album.as_deref(),
                Some("Random Access Memories"),
                "{name}"
            );
            assert_eq!(
                details.musicbrainz_recording, before.musicbrainz_recording,
                "{name}"
            );
            assert!(details.has_cover, "{name}");
            // The old artist's MusicBrainz ID no longer fits the new credit.
            let file = Probe::open(&path).unwrap().read().unwrap();
            let tag = file.primary_tag().unwrap();
            assert!(
                tag.get_string(ItemKey::MusicBrainzArtistId).is_none(),
                "{name}"
            );
        }

        let wav = directory.path().join("song.wav");
        std::fs::write(&wav, b"RIFF").unwrap();
        assert!(edit_fields(&wav, &TagEdit::default()).is_err());
    }

    #[test]
    fn telegram_details_replace_channel_tags() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("u6.mp3");
        let status = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("sine=frequency=440:duration=0.2")
            .args(["-map_metadata", "-1"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(
            apply_telegram_details(
                &path,
                Some("Fly me to the moon"),
                Some("The Macarons Project")
            )
            .unwrap()
        );
        let read = |path: &Path| {
            let file = Probe::open(path).unwrap().read().unwrap();
            let tag = file.primary_tag().unwrap();
            (
                tag.title().map(|value| value.into_owned()),
                tag.artist().map(|value| value.into_owned()),
            )
        };
        assert_eq!(
            read(&path),
            (
                Some("Fly me to the moon".into()),
                Some("The Macarons Project".into())
            )
        );
        // Tags that are there already win over Telegram's.
        // A channel's name from Telegram never replaces a real artist.
        assert!(!apply_telegram_details(&path, None, Some("@MusicBox")).unwrap());
        assert_eq!(read(&path).1.as_deref(), Some("The Macarons Project"));
    }

    #[test]
    fn an_artist_is_replaced_in_the_tags_only() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        for (name, codec) in [
            ("duet.mp3", "libmp3lame"),
            ("duet.flac", "flac"),
            ("duet.m4a", "aac"),
        ] {
            let path = directory.path().join(name);
            let status = std::process::Command::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
                .arg("sine=frequency=440:duration=0.2")
                .args(["-c:a", codec])
                .arg(&path)
                .status()
                .unwrap();
            assert!(status.success());
            let candidate = Candidate {
                title: "Song".into(),
                artist: "Ebi & Shadmehr Aghili".into(),
                album: Some("Song".into()),
                album_artist: Some("Ebi & Shadmehr Aghili".into()),
                ..Default::default()
            };
            write(&path, &candidate, None, None).unwrap();

            // The owner splits the combined artist into two.
            let parts = ["Ebi".to_owned(), "Shadmehr Aghili".to_owned()];
            assert!(replace_artist(&path, "Ebi & Shadmehr Aghili", &parts).unwrap());
            let file = Probe::open(&path).unwrap().read().unwrap();
            let tag = file.primary_tag().unwrap();
            let artists = tag.get_strings(ItemKey::TrackArtists).collect::<Vec<_>>();
            assert_eq!(artists, ["Ebi", "Shadmehr Aghili"], "{name}");
            assert_eq!(tag.artist().as_deref(), Some("Ebi & Shadmehr Aghili"));
            assert_eq!(tag.title().as_deref(), Some("Song"));

            // Renaming one of them changes only that one.
            assert!(replace_artist(&path, "Ebi", &["Ebi (Ebrahim Hamedi)".to_owned()]).unwrap());
            let file = Probe::open(&path).unwrap().read().unwrap();
            let tag = file.primary_tag().unwrap();
            let artists = tag.get_strings(ItemKey::TrackArtists).collect::<Vec<_>>();
            assert_eq!(
                artists,
                ["Ebi (Ebrahim Hamedi)", "Shadmehr Aghili"],
                "{name}"
            );
            assert!(!replace_artist(&path, "Googoosh", &["X".to_owned()]).unwrap());
        }
    }

    #[test]
    fn lyrics_go_inside_every_format_and_are_kept_on_retag() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let synced = "[00:01.00] first line\n[00:02.50] second line";
        for (name, codec) in [
            ("song.mp3", "libmp3lame"),
            ("song.flac", "flac"),
            ("song.m4a", "aac"),
            ("song.opus", "libopus"),
        ] {
            let path = directory.path().join(name);
            let status = std::process::Command::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
                .arg("sine=frequency=440:duration=0.2")
                .args(["-c:a", codec])
                .arg(&path)
                .status()
                .unwrap();
            if !status.success() {
                continue; // this ffmpeg lacks the encoder
            }
            let candidate = Candidate {
                title: "Song".into(),
                artist: "Artist".into(),
                ..Default::default()
            };
            write(&path, &candidate, Some(one_pixel_png()), Some(synced)).unwrap();
            verify_lyrics(&path, synced).unwrap();

            // Retagging without new lyrics keeps the ones inside.
            write(&path, &candidate, None, None).unwrap();
            assert_eq!(
                read_lyrics(&path).unwrap().as_deref(),
                Some(synced),
                "{name}"
            );
        }
    }

    #[test]
    fn other_catalog_ids_never_become_musicbrainz_tags() {
        assert_eq!(musicbrainz_id(&Some("1440857781".into())), None);
        assert_eq!(musicbrainz_id(&Some("discogs:123".into())), None);
        let mbid = "b10bbbfc-cf9e-42e0-be17-e2c3e1d2600d";
        assert_eq!(musicbrainz_id(&Some(mbid.into())).as_deref(), Some(mbid));
    }

    #[test]
    fn flac_cover_is_a_native_picture_block_after_disk_round_trip() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("native-cover.flac");
        let status = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("sine=frequency=440:duration=0.1")
            .args(["-c:a", "flac"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());

        let candidate = Candidate {
            title: "FLAC artwork test".into(),
            artist: "Test artist".into(),
            ..Default::default()
        };
        let artwork = one_pixel_png();
        write(&path, &candidate, Some(artwork.clone()), None).unwrap();

        assert_eq!(
            native_flac_front_cover(&path).unwrap().as_deref(),
            Some(artwork.as_slice())
        );
        verify_embedded_artwork(&path, &artwork).unwrap();
    }

    #[test]
    fn release_type_is_written_in_the_spelling_navidrome_reads() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        for (name, codec) in [("single.flac", "flac"), ("single.m4a", "aac")] {
            let path = directory.path().join(name);
            let status = std::process::Command::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
                .arg("sine=frequency=440:duration=0.2")
                .args(["-c:a", codec])
                .arg(&path)
                .status()
                .unwrap();
            assert!(status.success());
            let candidate = Candidate {
                title: "Houdini".into(),
                artist: "Dua Lipa".into(),
                album: Some("Houdini".into()),
                release_type: Some("Single".into()),
                ..Default::default()
            };
            write(&path, &candidate, Some(one_pixel_png()), None).unwrap();

            let file = Probe::open(&path).unwrap().read().unwrap();
            let tag = file.primary_tag().unwrap();
            assert_eq!(
                tag.get_string(ItemKey::MusicBrainzReleaseType),
                Some("single"),
                "{name}"
            );
        }
    }

    #[test]
    fn compilations_are_tagged_as_compilation_release_type() {
        let candidate = Candidate {
            release_type: Some("album".into()),
            is_compilation: true,
            ..Default::default()
        };
        assert_eq!(release_type_tag(&candidate), Some("compilation"));
        let unknown = Candidate {
            release_type: Some("something".into()),
            ..Default::default()
        };
        assert_eq!(release_type_tag(&unknown), None);
    }

    #[test]
    fn detects_mp4_container_hidden_behind_mp3_extension() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let m4a = directory.path().join("actual.m4a");
        let disguised = directory.path().join("wrong.mp3");
        let status = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("sine=frequency=440:duration=0.1")
            .args(["-c:a", "aac"])
            .arg(&m4a)
            .status()
            .unwrap();
        assert!(status.success());
        std::fs::rename(m4a, &disguised).unwrap();

        assert_eq!(
            detected_container_extension(&disguised).as_deref(),
            Some("m4a")
        );
        let file = Probe::open(&disguised)
            .unwrap()
            .guess_file_type()
            .unwrap()
            .read()
            .unwrap();
        assert_eq!(file.file_type(), lofty::file::FileType::Mp4);
    }

    #[test]
    fn navidrome_artist_fields_survive_supported_format_round_trips() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        for (extension, codec_args) in [
            ("mp3", vec!["-q:a", "9"]),
            ("flac", vec!["-c:a", "flac"]),
            ("m4a", vec!["-c:a", "aac", "-b:a", "64k"]),
            (
                "ogg",
                vec!["-ac", "2", "-c:a", "vorbis", "-strict", "-2", "-q:a", "2"],
            ),
            ("opus", vec!["-c:a", "libopus", "-b:a", "64k"]),
        ] {
            let path = directory.path().join(format!("artists.{extension}"));
            let status = std::process::Command::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
                .arg("sine=frequency=440:duration=0.1")
                .args(codec_args)
                .arg(&path)
                .status()
                .unwrap();
            assert!(status.success(), "could not create {extension} fixture");

            let candidate = Candidate {
                title: "Sunshine".into(),
                artist: "Alice & Bob".into(),
                artist_credits: vec![
                    crate::domain::credits::ArtistCredit {
                        name: "Alice".into(),
                        join_phrase: " & ".into(),
                        musicbrainz_id: Some("mbid-alice".into()),
                        artist_id: None,
                    },
                    crate::domain::credits::ArtistCredit {
                        name: "Bob".into(),
                        join_phrase: String::new(),
                        musicbrainz_id: Some("mbid-bob".into()),
                        artist_id: None,
                    },
                ],
                album: Some("Compilation".into()),
                album_artist: Some("Various Artists".into()),
                album_artist_credits: vec![crate::domain::credits::ArtistCredit::new(
                    "Various Artists",
                    "",
                )],
                is_compilation: true,
                ..Default::default()
            };
            write(&path, &candidate, None, None).unwrap();
            verify_written_metadata(&path, &candidate)
                .unwrap_or_else(|error| panic!("{extension}: {error:#}"));

            let band = Candidate {
                title: "Love You Like a Love Song".into(),
                artist: "Selena Gomez & the Scene".into(),
                artist_credits: vec![crate::domain::credits::ArtistCredit::new(
                    "Selena Gomez & the Scene",
                    "",
                )],
                album: Some("When the Sun Goes Down".into()),
                album_artist: Some("Selena Gomez & the Scene".into()),
                album_artist_credits: vec![crate::domain::credits::ArtistCredit::new(
                    "Selena Gomez & the Scene",
                    "",
                )],
                ..Default::default()
            };
            write(&path, &band, None, None).unwrap();
            verify_written_metadata(&path, &band)
                .unwrap_or_else(|error| panic!("{extension} band identity: {error:#}"));
        }
    }

    #[test]
    fn dates_are_written_in_navidrome_shape_only() {
        assert_eq!(
            full_date(&Some("2024-03-15".into())).as_deref(),
            Some("2024-03-15")
        );
        assert_eq!(
            full_date(&Some("2024-03".into())).as_deref(),
            Some("2024-03")
        );
        assert_eq!(full_date(&Some("2024".into())).as_deref(), Some("2024"));
        assert_eq!(full_date(&Some("0000-00-00".into())), None);
        assert_eq!(full_date(&Some("March 2024".into())), None);
        assert_eq!(full_date(&None), None);
    }

    #[test]
    fn big_covers_are_shrunk_to_jpeg_and_fitting_jpegs_are_kept() {
        let big = image::RgbImage::from_fn(3000, 3000, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
        });
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(big)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let prepared = prepare_cover(&png.into_inner()).unwrap();
        let (width, height) = image::ImageReader::new(std::io::Cursor::new(&prepared))
            .with_guessed_format()
            .unwrap()
            .into_dimensions()
            .unwrap();
        assert_eq!((width, height), (1200, 1200));
        assert_eq!(
            image::guess_format(&prepared).unwrap(),
            image::ImageFormat::Jpeg
        );

        let small =
            image::RgbImage::from_fn(600, 600, |x, _| image::Rgb([(x % 256) as u8, 20, 20]));
        let mut jpeg = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(small)
            .write_to(&mut jpeg, image::ImageFormat::Jpeg)
            .unwrap();
        let jpeg = jpeg.into_inner();
        assert_eq!(prepare_cover(&jpeg).unwrap(), jpeg);
    }

    #[test]
    fn m4a_multi_values_live_in_one_atom() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("duet.m4a");
        let status = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("sine=frequency=440:duration=0.1")
            .args(["-c:a", "aac", "-b:a", "64k"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success());
        let duet = Candidate {
            title: "Khabe Khoob".into(),
            artist: "Ebi & Shadmehr Aghili".into(),
            artist_credits: vec![
                crate::domain::credits::ArtistCredit::new("Ebi", " & "),
                crate::domain::credits::ArtistCredit::new("Shadmehr Aghili", ""),
            ],
            ..Default::default()
        };
        write(&path, &duet, None, None).unwrap();
        verify_written_metadata(&path, &duet).unwrap();
        use lofty::config::ParseOptions;
        use lofty::file::AudioFile;
        let mut file = std::fs::File::open(&path).unwrap();
        let mp4 = lofty::mp4::Mp4File::read_from(&mut file, ParseOptions::new()).unwrap();
        let idents = mp4
            .ilst()
            .unwrap()
            .into_iter()
            .map(|atom| format!("{:?}", atom.ident()))
            .collect::<Vec<_>>();
        let artists_atoms = idents
            .iter()
            .filter(|ident| ident.contains("name: \"ARTISTS\""))
            .count();
        assert_eq!(artists_atoms, 1, "{idents:?}");
        assert_eq!(
            mp4_values(mp4.ilst().unwrap(), ItemKey::TrackArtists),
            vec!["Ebi".to_owned(), "Shadmehr Aghili".to_owned()]
        );
    }

    #[test]
    fn artwork_validation_rejects_tiny_transparent_and_blank_placeholders() {
        let tiny = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
        )
        .unwrap();
        assert!(validate_artwork(&tiny).is_err());

        let encode = |pixel| {
            let image = image::RgbaImage::from_pixel(320, 320, pixel);
            let mut output = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(image)
                .write_to(&mut output, image::ImageFormat::Png)
                .unwrap();
            output.into_inner()
        };
        let transparent = encode(image::Rgba([0, 0, 0, 0]));
        let constant = encode(image::Rgba([255, 255, 255, 255]));
        assert!(validate_artwork(&transparent).is_err());
        assert!(validate_artwork(&constant).is_err());
        assert!(inspect_artwork(&constant, true).is_ok());
        assert!(validate_artwork(&test_artwork_png()).is_ok());
    }
}
