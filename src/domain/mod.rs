pub mod audio;
pub mod credits;
pub mod genre;
pub mod matcher;

/// How a provider id is shown to people ("itunes" → "Apple Music").
/// Unknown ids are shown as they are.
pub fn provider_name(provider: &str) -> &str {
    match provider {
        "" | "musicbrainz" => "MusicBrainz",
        "acoustid" => "AcoustID",
        "audd" => "AudD",
        "audiomack" => "Audiomack",
        "deezer" => "Deezer",
        "discogs" => "Discogs",
        "genius" => "Genius",
        "itunes" => "Apple Music",
        "lastfm" => "Last.fm",
        "navahang" => "Navahang",
        "radiojavan" => "Radio Javan",
        "shazam" => "Shazam",
        "songrec" => "SongRec / Shazam",
        "soundcloud" => "SoundCloud",
        "spotify" => "Spotify",
        "youtube" => "YouTube",
        other => other,
    }
}
