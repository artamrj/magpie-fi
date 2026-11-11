//! Whole journeys through the API, as the web page and the bot drive it: a
//! song goes from Review into the library, the same song comes a second time,
//! its tags are edited, its artist is renamed. Everything runs on a temporary
//! music folder with the real router, the real database and real audio files.
//!
//! Nothing here may reach the internet: the HTTP client points at a closed
//! port, so a lookup that slipped in would fail instead of leaving the machine.
//! Identification itself (which needs the online sources) is not part of
//! these tests; a song starts where a scan leaves it, waiting in Review.

use crate::{
    app::AppState,
    config::Config,
    http::router::router,
    infrastructure::{db, media::tag_writer, providers::Candidate},
    types::WorkflowPhase,
};
use axum::{
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode, header},
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tower::ServiceExt;

struct Install {
    _directory: tempfile::TempDir,
    state: Arc<AppState>,
}

impl Install {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let config = Config::with_folders(
            &directory.path().join("music"),
            &directory.path().join("data"),
        );
        for folder in [&config.input_dir, &config.output_dir] {
            std::fs::create_dir_all(folder).unwrap();
        }
        let pool = db::connect(&config.db_path).await.unwrap();
        let mut state = AppState::new(config, pool);
        state.client = reqwest::Client::builder()
            .proxy(reqwest::Proxy::all("http://127.0.0.1:9").unwrap())
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        Self {
            _directory: directory,
            state: Arc::new(state),
        }
    }

    fn inbox(&self) -> PathBuf {
        PathBuf::from(&self.state.config.input_dir)
    }

    fn library(&self) -> PathBuf {
        PathBuf::from(&self.state.config.output_dir)
    }

    async fn call(&self, method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let request = Request::builder().method(method).uri(uri);
        let request = match body {
            Some(body) => request
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string())),
            None => request.body(Body::empty()),
        }
        .unwrap();
        let response = router()
            .with_state(self.state.clone())
            .oneshot(request)
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn get(&self, uri: &str) -> Value {
        let (status, body) = self.call(Method::GET, uri, None).await;
        assert_eq!(status, StatusCode::OK, "GET {uri}: {body}");
        body
    }

    async fn post(&self, uri: &str, body: Option<Value>) -> Value {
        let (status, body) = self.call(Method::POST, uri, body).await;
        assert_eq!(status, StatusCode::OK, "POST {uri}: {body}");
        body
    }

    /// Waits until the running job is over and says how it ended.
    async fn wait_until_idle(&self) -> WorkflowPhase {
        for _ in 0..600 {
            if !self.state.workflow_running().await {
                return self.state.workflow.read().await.phase;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the job never finished");
    }

    /// Files everything that is Ready, as "File now" does.
    async fn file_now(&self) {
        let started = self.post("/write", None).await;
        assert_eq!(started["started"], true);
        assert_eq!(self.wait_until_idle().await, WorkflowPhase::Finish);
    }

    /// A song as a scan leaves it when it is unsure: in the inbox, waiting in Review.
    async fn song_in_review(&self, file: &Path, content_key: &str) -> i64 {
        let info = crate::domain::audio::read(file).unwrap();
        sqlx::query(
            "INSERT INTO tracks(path,filename,format,duration,content_fingerprint,status,is_missing,
               first_seen_at,last_seen_at,last_scanned_at,stage,
               current_title,current_artist,current_album)
             VALUES(?,?,?,?,?,'needs_review',0,'now','now','now','review',?,?,?)",
        )
        .bind(file.to_string_lossy().as_ref())
        .bind(file.file_name().unwrap().to_string_lossy().as_ref())
        .bind(&info.format)
        .bind(info.duration)
        .bind(content_key)
        .bind(&info.title)
        .bind(&info.artist)
        .bind(&info.album)
        .execute(&self.state.pool)
        .await
        .unwrap()
        .last_insert_rowid()
    }

    fn library_songs(&self) -> Vec<PathBuf> {
        let mut songs = crate::application::files::audio_files(&self.library()).0;
        songs.sort();
        songs
    }

    fn trashed(&self) -> Vec<PathBuf> {
        crate::application::files::audio_files(&self.state.config.trash_dir).0
    }

    fn relative(&self, song: &Path) -> String {
        song.strip_prefix(self.library())
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }
}

/// A short tagged MP3 with a real square cover inside, or `None` without ffmpeg.
fn tagged_song(path: &Path, title: &str, artist: &str, album: &str, tone: u32) -> Option<()> {
    let made = std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
        .arg(format!("sine=frequency={tone}:duration=3"))
        .args(["-q:a", "9"])
        .arg(path)
        .status()
        .ok()?;
    assert!(made.success());
    let tags = Candidate {
        title: title.into(),
        artist: artist.into(),
        album: Some(album.into()),
        album_artist: Some(artist.into()),
        year: Some("1999".into()),
        ..Default::default()
    };
    tag_writer::write(
        path,
        &tags,
        Some(tag_writer::test_artwork_png_sized(600, 600)),
        None,
    )
    .unwrap();
    Some(())
}

fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

fn hidden_files(folder: &Path) -> Vec<PathBuf> {
    walkdir::WalkDir::new(folder)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().is_file() && entry.file_name().to_string_lossy().starts_with('.')
        })
        .map(walkdir::DirEntry::into_path)
        .collect()
}

#[tokio::test]
async fn a_fresh_install_answers_every_page_and_has_nothing_to_do() {
    let install = Install::new().await;
    assert_eq!(install.get("/health").await["status"], "ok");
    assert_eq!(install.get("/status").await["phase"], "idle");
    for list in ["/tracks", "/artists", "/requests", "/library/recent"] {
        assert_eq!(install.get(list).await, json!([]), "{list}");
    }
    let root = install.get("/files").await;
    assert_eq!(root["songs_inside"], 0);
    assert!(install.get("/setup").await.is_object());
    assert_eq!(
        install.get("/files/search?q=anything").await["songs"],
        json!([])
    );

    // Nothing to file, nothing to approve, nothing to repair: each says so
    // and leaves the "one job at a time" slot free for the next.
    let (status, _) = install.call(Method::POST, "/write", None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    install.post("/tracks/auto-approve", None).await;
    assert_eq!(
        install.post("/tracks/retry-issues", None).await["started"],
        false
    );
    assert_eq!(install.post("/stop", None).await["stopping"], true);
    assert!(!install.state.workflow_running().await);

    // Things that do not exist are "not found", never a crash.
    for (method, uri) in [
        (Method::DELETE, "/tracks/999"),
        (Method::POST, "/tracks/999/keep-tags"),
        (Method::GET, "/files/song?path=Nobody%2FNothing.mp3"),
        (Method::GET, "/no-such-page"),
    ] {
        let (status, body) = install.call(method.clone(), uri, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}: {body}");
    }
    let (status, _) = install
        .call(Method::POST, "/tracks/999/file-anyway", None)
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(install.get("/requests/999").await, json!([]));
}

#[tokio::test]
async fn a_song_goes_from_review_into_the_library_and_stays_safe_there() {
    let install = Install::new().await;
    let inbox_file = install.inbox().join("ebi - hamdam.mp3");
    if tagged_song(&inbox_file, "Hamdam", "Ebi", "Khalij", 440).is_none() {
        return;
    }
    let original = std::fs::read(&inbox_file).unwrap();
    let track = install.song_in_review(&inbox_file, "fp:hamdam").await;

    // 1. It waits in Review, and nothing is filed while it does.
    let waiting = install.get("/tracks").await;
    assert_eq!(waiting.as_array().unwrap().len(), 1);
    assert_eq!(waiting[0]["stage"], "review");
    let (status, _) = install.call(Method::POST, "/write", None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(install.library_songs().is_empty());

    // 2. "Keep the file's own tags": its own cover is a real one, so it is Ready.
    let kept = install
        .post(&format!("/tracks/{track}/keep-tags"), None)
        .await;
    assert_eq!(kept["ready"], true, "{kept}");
    assert_eq!(kept["cover_verified"], true);

    // 3. File it.
    install.file_now().await;
    let songs = install.library_songs();
    assert_eq!(songs.len(), 1, "{songs:?}");
    let filed = songs[0].clone();
    let relative = install.relative(&filed);
    assert!(relative.starts_with("Ebi/Khalij"), "{relative}");
    assert!(relative.ends_with(".mp3"));
    let tags = tag_writer::read_details(&filed).unwrap();
    assert_eq!(tags.title.as_deref(), Some("Hamdam"));
    assert_eq!(tags.artist.as_deref(), Some("Ebi"));
    assert_eq!(tags.album.as_deref(), Some("Khalij"));
    // The cover is inside the song and next to it.
    let cover = router()
        .with_state(install.state.clone())
        .oneshot(
            Request::builder()
                .uri(format!("/files/artwork?path={}", encode(&relative)))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cover.status(), StatusCode::OK);
    assert!(filed.with_file_name("cover.jpg").exists());
    // The original is in the trash, byte for byte, not deleted.
    assert!(!inbox_file.exists());
    let trashed = install.trashed();
    assert_eq!(trashed.len(), 1);
    assert_eq!(std::fs::read(&trashed[0]).unwrap(), original);
    // Every page agrees.
    assert_eq!(install.get("/tracks").await, json!([]));
    let artists = install.get("/artists").await;
    assert_eq!(artists.as_array().unwrap().len(), 1);
    assert_eq!(artists[0]["name"], "Ebi");
    assert_eq!(artists[0]["songs"], 1);
    assert_eq!(install.get("/files").await["songs_inside"], 1);
    assert_eq!(install.get("/library/recent").await[0]["title"], "Hamdam");
    let song_uri = format!("/files/song?path={}", encode(&relative));
    let details = install.get(&song_uri).await;
    assert_eq!(details["title"], "Hamdam");

    // 4. The same song arrives a second time: the library keeps its one copy
    //    untouched and the new file goes to the trash.
    let filed_bytes = std::fs::read(&filed).unwrap();
    let again = install.inbox().join("hamdam again.mp3");
    std::fs::write(&again, &original).unwrap();
    let second = install.song_in_review(&again, "fp:hamdam").await;
    install
        .post(&format!("/tracks/{second}/keep-tags"), None)
        .await;
    let started = install.post("/write", None).await;
    assert_eq!(started["started"], true);
    install.wait_until_idle().await;
    assert_eq!(install.library_songs(), vec![filed.clone()]);
    assert_eq!(std::fs::read(&filed).unwrap(), filed_bytes);
    assert!(!again.exists());
    assert_eq!(install.trashed().len(), 2);
    assert_eq!(install.get("/tracks").await, json!([]));

    // 5. Fix a tag on the Files tab. The song is still whole afterwards.
    let length = crate::domain::audio::read(&filed).unwrap().duration;
    let (status, saved) = install
        .call(
            Method::PUT,
            "/files/song",
            Some(json!({
                "path": relative,
                "version": details["version"],
                "changes": {"title": "Hamdam (Live)", "genre": "Pop"},
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["changed"], 2);
    let edited = install.get(&song_uri).await;
    assert_eq!(edited["title"], "Hamdam (Live)");
    assert_eq!(edited["genre"], "Pop");
    assert_eq!(edited["artist"], "Ebi");
    let after = crate::domain::audio::read(&filed).unwrap();
    assert!((after.duration - length).abs() < 0.1);
    // An edit made from a stale page is refused, not written over the newer file.
    let (status, _) = install
        .call(
            Method::PUT,
            "/files/song",
            Some(json!({
                "path": relative,
                "version": details["version"],
                "changes": {"title": "Old page"},
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(install.get(&song_uri).await["title"], "Hamdam (Live)");

    // 6. Rename the artist: tags and folder follow, the song is not lost.
    let artist = artists[0]["id"].as_i64().unwrap();
    install
        .post(
            &format!("/artists/{artist}"),
            Some(json!({"action": "rename", "name": "Ebrahim Hamedi"})),
        )
        .await;
    let songs = install.library_songs();
    assert_eq!(songs.len(), 1);
    let renamed = install.relative(&songs[0]);
    assert!(renamed.starts_with("Ebrahim Hamedi/"), "{renamed}");
    assert_eq!(
        tag_writer::read_details(&songs[0])
            .unwrap()
            .artist
            .as_deref(),
        Some("Ebrahim Hamedi")
    );
    assert_eq!(install.get("/artists").await[0]["name"], "Ebrahim Hamedi");
    assert!(!install.library().join("Ebi").exists());

    // 7. Nothing is left running and no work file is left in the library.
    assert!(!install.state.workflow_running().await);
    assert_eq!(hidden_files(&install.library()), Vec::<PathBuf>::new());
    assert_eq!(install.inbox().read_dir().unwrap().count(), 0);
}

#[tokio::test]
async fn a_song_removed_in_review_goes_to_the_trash_and_nowhere_else() {
    let install = Install::new().await;
    let inbox_file = install.inbox().join("unwanted.mp3");
    if tagged_song(&inbox_file, "Unwanted", "Somebody", "Album", 500).is_none() {
        return;
    }
    let original = std::fs::read(&inbox_file).unwrap();
    let track = install.song_in_review(&inbox_file, "fp:unwanted").await;

    let (status, body) = install
        .call(Method::DELETE, &format!("/tracks/{track}"), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    assert!(!inbox_file.exists());
    assert!(install.library_songs().is_empty());
    let trashed = install.trashed();
    assert_eq!(trashed.len(), 1);
    assert_eq!(std::fs::read(&trashed[0]).unwrap(), original);
    assert_eq!(install.get("/tracks").await, json!([]));
}

#[tokio::test]
async fn a_damaged_file_shows_as_an_issue_and_never_as_a_server_error() {
    let install = Install::new().await;
    let broken = install.inbox().join("broken file.mp3");
    std::fs::write(&broken, vec![7_u8; 20_000]).unwrap();
    let track = sqlx::query(
        "INSERT INTO tracks(path,filename,status,is_missing,first_seen_at,last_seen_at,
           last_scanned_at,stage,stage_message)
         VALUES(?,'broken file.mp3','provider_error',0,'now','now','now','failed','Track failed')",
    )
    .bind(broken.to_string_lossy().as_ref())
    .execute(&install.state.pool)
    .await
    .unwrap()
    .last_insert_rowid();

    assert_eq!(install.get("/tracks").await[0]["stage"], "failed");
    // Everything the page asks about this song gets a plain answer.
    for uri in [
        format!("/tracks/{track}/artwork/original"),
        format!("/tracks/{track}/artwork/preview"),
    ] {
        let (status, body) = install.call(Method::GET, &uri, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri}: {body}");
    }
    let (status, body) = install
        .call(Method::POST, &format!("/tracks/{track}/keep-tags"), None)
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (status, body) = install.call(Method::POST, "/write", None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    // It is never filed and never removed behind your back.
    assert!(broken.exists());
    assert!(install.library_songs().is_empty());
    assert!(!install.state.workflow_running().await);
}

#[tokio::test]
async fn a_look_alike_is_asked_about_and_filed_next_to_the_other_on_request() {
    let install = Install::new().await;
    // A song you already own, indexed with its ISRC.
    let owned = install.library().join("Ebi/Khalij (1999)/01 - Hamdam.mp3");
    std::fs::create_dir_all(owned.parent().unwrap()).unwrap();
    if tagged_song(&owned, "Hamdam", "Ebi", "Khalij", 440).is_none() {
        return;
    }
    let owned_bytes = std::fs::read(&owned).unwrap();
    crate::application::library::index_library(&install.state.pool, &install.library())
        .await
        .unwrap();
    sqlx::query("UPDATE library_items SET isrc='USUM71703861'")
        .execute(&install.state.pool)
        .await
        .unwrap();

    // Another recording of it (it sounds different), whose match carries that ISRC.
    let inbox_file = install.inbox().join("hamdam remake.mp3");
    tagged_song(&inbox_file, "Hamdam", "Ebi", "Khalij", 880).unwrap();
    let track = install.song_in_review(&inbox_file, "fp:remake").await;
    install
        .post(&format!("/tracks/{track}/keep-tags"), None)
        .await;
    sqlx::query("UPDATE candidates SET isrc='USUM71703861' WHERE track_id=?")
        .bind(track)
        .execute(&install.state.pool)
        .await
        .unwrap();

    // Filing does not touch either file: it asks.
    install.post("/write", None).await;
    install.wait_until_idle().await;
    assert_eq!(install.library_songs(), vec![owned.clone()]);
    assert_eq!(std::fs::read(&owned).unwrap(), owned_bytes);
    assert!(inbox_file.exists());
    assert!(install.trashed().is_empty());
    let waiting = install.get("/tracks").await;
    assert_eq!(waiting[0]["stage"], "review");
    assert!(
        waiting[0]["stage_message"]
            .as_str()
            .unwrap()
            .starts_with("Possible duplicate"),
        "{waiting}"
    );

    // "File anyway": both are in the library, the first one unchanged.
    install
        .post(&format!("/tracks/{track}/file-anyway"), None)
        .await;
    install.file_now().await;
    let songs = install.library_songs();
    assert_eq!(songs.len(), 2, "{songs:?}");
    assert_eq!(std::fs::read(&owned).unwrap(), owned_bytes);
    assert_eq!(install.get("/tracks").await, json!([]));
}

#[tokio::test]
async fn nothing_else_starts_while_a_job_runs_and_everything_works_after() {
    let install = Install::new().await;
    let song = install.library().join("Artist/Album/01 - Song.mp3");
    std::fs::create_dir_all(song.parent().unwrap()).unwrap();
    if tagged_song(&song, "Song", "Artist", "Album", 440).is_none() {
        return;
    }
    let before = std::fs::read(&song).unwrap();
    crate::application::library::index_library(&install.state.pool, &install.library())
        .await
        .unwrap();
    let artist = install.get("/artists").await[0]["id"].as_i64().unwrap();
    let details = install
        .get("/files/song?path=Artist%2FAlbum%2F01%20-%20Song.mp3")
        .await;
    let edit = json!({
        "path": "Artist/Album/01 - Song.mp3",
        "version": details["version"],
        "changes": {"title": "Changed"},
    });

    // A scan is running.
    assert!(
        install
            .state
            .try_claim_workflow(WorkflowPhase::Scan, "Scanning")
            .await
    );
    for (method, uri, body) in [
        (Method::PUT, "/files/song", Some(edit.clone())),
        (
            Method::PUT,
            "/files/songs",
            Some(json!({"edits": [edit.clone()]})),
        ),
        (
            Method::POST,
            &*format!("/artists/{artist}"),
            Some(json!({"action": "rename", "name": "Other"})),
        ),
        (Method::POST, "/tracks/auto-approve", None),
        (Method::POST, "/tracks/retry-issues", None),
    ] {
        let (status, answer) = install.call(method.clone(), uri, body).await;
        assert_eq!(status, StatusCode::CONFLICT, "{method} {uri}: {answer}");
    }
    assert_eq!(std::fs::read(&song).unwrap(), before, "nothing was written");
    assert_eq!(install.get("/status").await["phase"], "scan");

    // The scan ends; the same edit now goes through.
    install
        .state
        .reset_workflow(WorkflowPhase::Idle, "Ready")
        .await;
    let (status, saved) = install.call(Method::PUT, "/files/song", Some(edit)).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["song"]["title"], "Changed");
    assert!(!install.state.workflow_running().await);
}
