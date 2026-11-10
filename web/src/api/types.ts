/** Read-only configuration. It comes from MAGPIE_* settings in the server's .env file. */
export type Setup = {
  inbox_dir: string;
  library_dir: string;
  trash_dir: string;
  trash_days: number;
  inbox_check_seconds: number;
  lyrics: boolean;
  rsgain: boolean;
  library_songs: number;
  ytdlp: boolean;
  audio_format: string;
  max_parallel_downloads: number;
  navidrome: { configured: boolean; url?: string; reachable?: boolean };
  telegram: { configured: boolean; owners?: number; guests?: number; large_files?: boolean };
  sources: Record<string, boolean>;
};
/** A link someone asked for (GET /api/requests). */
export type MusicRequest = {
  id: number;
  url: string;
  kind: "track" | "album" | "playlist" | "upload";
  title?: string;
  owner?: string;
  status: "reading" | "ready" | "failed";
  error?: string;
  created_at: string;
  total: number;
  done: number;
  working: number;
  review: number;
  failed: number;
};
/** One song of a request (GET /api/requests/{id}). */
export type WantedSong = {
  position: number;
  title: string;
  artist?: string;
  status: string;
  error?: string;
  library_path?: string;
  attempts: number;
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
  artwork_candidates?: ArtworkCandidate[];
  artwork_status?: "searching" | "verified" | "retryable_error" | "cover_required";
  artwork_message?: string;
  score_breakdown?: string;
  score: number;
};
export type ArtistCredit = {
  name: string;
  join_phrase: string;
  musicbrainz_id?: string;
};
export type ArtworkCandidate = {
  provider: string;
  url: string;
  user_confirmed: boolean;
  release_id?: string;
  isrc?: string;
  album?: string;
  artist?: string;
};
export type TrackStatus =
  | "new"
  | "selected"
  | "needs_review"
  | "review"
  | "applied"
  | "failed"
  | "corrupt"
  | "provider_error"
  | "processing"
  | "duplicate"
  | "verified"
  | "retryable_error";
export type TrackStage = "discovered" | "selected" | "ready" | "review" | "skipped" | "failed";
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
export type TrackPage = { items: Track[]; total: number };
export type AutoApproveResult = {
  approved: number;
  remaining: number;
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
  matched: number;
  unmatched: number;
  failed: number;
};
