/** Read-only configuration. It comes from MAGPIE_* settings in the server's .env file. */
export type Setup = {
  inbox_dir: string;
  library_dir: string;
  trash_dir: string;
  trash_days: number;
  inbox_check_seconds: number;
  lyrics: boolean;
  sidecars: boolean;
  rsgain: boolean;
  library_songs: number;
  ytdlp: boolean;
  audio_format: string;
  max_parallel_downloads: number;
  telegram: { configured: boolean; owners?: number; guests?: number };
  sources: Record<string, boolean>;
};
/** Who is using the page (GET /api/me). */
/** An artist in the library: one name for all its spellings. */
export type ArtistSummary = {
  id: number;
  name: string;
  spellings: string[];
  songs: number;
  musicbrainz_id?: string | null;
  single_entity: boolean;
  looks_like_several: boolean;
};

/** What an artist change did to the songs. */
export type ArtistChanged = { retagged: number; moved: number };

export type Me = {
  role: "owner" | "guest";
  via: "telegram" | "session" | "local";
};
/** A song filed recently (GET /api/library/recent). */
export type RecentSong = {
  title?: string;
  artist?: string;
  album?: string;
  added_at: string;
};
/** A link someone asked for (GET /api/requests). */
export type MusicRequest = {
  id: number;
  url: string;
  kind: "track" | "album" | "playlist" | "upload";
  title?: string;
  status: "reading" | "ready" | "failed";
  /** Why the link could not be read. */
  error?: string;
  total: number;
  done: number;
  working: number;
  review: number;
  failed: number;
  /** Removed by the owner in Review. */
  removed: number;
  created_at: string;
  updated_at: string;
  /** The Review track of the first song waiting for the owner. */
  track_id?: number | null;
  /** The first song's error, for a request of one song. */
  first_error?: string | null;
};
/** One song of a request (GET /api/requests/{id}). */
export type WantedSong = {
  position: number;
  title: string;
  artist?: string;
  status: string;
  error?: string;
  track_id?: number | null;
  updated_at: string;
};
export type Candidate = {
  id: number;
  provider?: string;
  title?: string;
  artist?: string;
  artist_credits?: ArtistCredit[];
  album?: string;
  album_artist?: string;
  album_artist_credits?: ArtistCredit[];
  track_number?: number;
  track_total?: number;
  disc_number?: number;
  disc_total?: number;
  year?: string;
  genre?: string;
  composer?: string;
  label?: string;
  isrc?: string;
  release_date?: string;
  cover_url?: string;
  artwork_status: ArtworkStatus;
  artwork_message?: string;
  score_breakdown?: string;
  score: number;
};
export type ArtworkStatus = "searching" | "verified" | "retryable_error" | "cover_required";
export type ArtistCredit = {
  name: string;
  join_phrase: string;
  musicbrainz_id?: string;
};
export type TrackStatus =
  | "new"
  | "selected"
  | "needs_review"
  | "review"
  | "failed"
  | "corrupt"
  | "provider_error"
  | "processing"
  | "duplicate"
  | "verified"
  | "retryable_error";
export type TrackStage = "discovered" | "ready" | "review" | "skipped" | "failed";
export type Track = {
  id: number;
  filename: string;
  format?: string;
  duration?: number;
  current_title?: string;
  current_artist?: string;
  current_album?: string;
  current_album_artist?: string;
  current_track_number?: number;
  selected_candidate_id?: number;
  status: TrackStatus;
  stage: TrackStage;
  stage_message?: string;
  error?: string;
  is_missing: boolean;
  candidates: Candidate[];
};
export type AutoApproveResult = {
  approved: number;
  low_confidence: number;
  unavailable: number;
};
export type RetryIssuesResult = { started: boolean; queued: number; unavailable: number };
export type Workflow = {
  phase: "idle" | "scan" | "fetch" | "preview" | "apply" | "finish" | "failed";
  message: string;
  current_file?: string;
  current: number;
  total: number;
  processed: number;
};
