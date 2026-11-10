import type { ArtworkStatus, Candidate, Track } from "../api/types";
import type { IconName } from "./Icons";

export const busyPhases = new Set(["scan", "fetch", "apply"]);
export type QueueFilter = "all" | "review" | "problems" | "ready";
export type QueueOrder = "queue" | "title" | "artist" | "status";
export type Theme = "dark" | "light";

export const queueOrderLabels: Record<QueueOrder, string> = {
  queue: "Queue",
  title: "Title",
  artist: "Artist",
  status: "Status",
};
export const queueOrderSequence: QueueOrder[] = ["queue", "title", "artist", "status"];

export function candidateConfidence(score: number) {
  if (score >= 85) return { label: "Strong match", tone: "strong" };
  if (score >= 70) return { label: "Likely match", tone: "likely" };
  if (score >= 55) return { label: "Uncertain", tone: "uncertain" };
  return { label: "Weak match", tone: "weak" };
}

type Evidence = {
  title?: unknown;
  artist?: unknown;
  album_context?: unknown;
  duration?: unknown;
  sources?: unknown;
  metadata_completion?: { score?: unknown; core_complete?: unknown; missing_fields?: unknown };
};
const evidenceCache = new WeakMap<Candidate, Evidence>();

/** The parsed `score_breakdown` of a candidate, parsed once per candidate object. */
function evidenceOf(candidate: Candidate): Evidence {
  let evidence = evidenceCache.get(candidate);
  if (!evidence) {
    evidence = parseEvidence(candidate.score_breakdown);
    evidenceCache.set(candidate, evidence);
  }
  return evidence;
}
function parseEvidence(raw?: string): Evidence {
  try {
    return JSON.parse(raw || "{}") ?? {};
  } catch {
    return {};
  }
}

export function candidateSignals(track: Track, candidate: Candidate) {
  const evidence = evidenceOf(candidate);
  return [
    comparisonSignal("Title", track.current_title, candidate.title, evidence.title),
    comparisonSignal("Artist", track.current_artist, candidate.artist, evidence.artist),
    comparisonSignal("Album", track.current_album, candidate.album, evidence.album_context),
    durationSignal(evidence.duration),
  ];
}

export function candidateVerdict(score: number, signals: Array<{ tone: string }>) {
  const warnings = signals.filter((signal) => signal.tone === "warning").length;
  if (score >= 85 && warnings === 0)
    return {
      tone: "good",
      title: "Good choice",
      detail: "The identity and audio evidence agree with your file.",
    };
  if (score >= 70 && warnings <= 1)
    return {
      tone: "caution",
      title: "Likely match",
      detail: "Most evidence agrees; check the highlighted difference before choosing.",
    };
  return {
    tone: "risk",
    title: "Needs caution",
    detail: warnings
      ? `${warnings} important ${warnings === 1 ? "difference" : "differences"} detected. Choose only if you recognize this release.`
      : "The available evidence is too weak to recommend this automatically.",
  };
}

function comparisonSignal(
  label: string,
  original?: string,
  proposed?: string,
  rawSimilarity?: unknown,
) {
  if (!original?.trim()) return { label, value: "No original", tone: "muted" };
  if (!proposed?.trim()) return { label, value: "Missing", tone: "warning" };
  const similarity =
    typeof rawSimilarity === "number"
      ? rawSimilarity
      : normalizeText(original) === normalizeText(proposed)
        ? 1
        : 0;
  if (similarity >= 0.92) return { label, value: "Exact", tone: "good" };
  if (similarity >= 0.7) return { label, value: "Close", tone: "good" };
  if (similarity >= 0.4) return { label, value: "Different", tone: "warning" };
  return { label, value: "Mismatch", tone: "warning" };
}

function durationSignal(rawSimilarity?: unknown) {
  if (typeof rawSimilarity !== "number")
    return { label: "Audio length", value: "Not checked", tone: "muted" };
  if (rawSimilarity >= 0.9) return { label: "Audio length", value: "Same", tone: "good" };
  if (rawSimilarity >= 0.55) return { label: "Audio length", value: "Close", tone: "good" };
  return { label: "Audio length", value: "Different", tone: "warning" };
}

function normalizeText(value: string) {
  return value
    .toLocaleLowerCase()
    .replace(/[^\p{L}\p{N}]+/gu, "")
    .trim();
}

export const artworkStatusLabels: Record<ArtworkStatus, string> = {
  verified: "Verified cover",
  searching: "Checking cover",
  retryable_error: "Cover source unavailable",
  cover_required: "Cover required",
};

export function formatDuration(seconds?: number) {
  if (!seconds || !Number.isFinite(seconds)) return undefined;
  const minutes = Math.floor(seconds / 60);
  return `${minutes}:${Math.round(seconds % 60)
    .toString()
    .padStart(2, "0")}`;
}

export function isReview(track: Track) {
  return track.stage === "review" && !isProblem(track);
}
export function isReady(track: Track) {
  return track.stage === "ready" && Boolean(track.selected_candidate_id);
}
export function isProblem(track: Track) {
  if (track.stage === "skipped") return false;
  return (
    track.status === "corrupt" ||
    track.is_missing ||
    track.stage === "failed" ||
    track.status === "failed" ||
    track.status === "provider_error"
  );
}
export function hasRetryableArtwork(track: Track) {
  return selectedCandidate(track)?.artwork_status === "retryable_error";
}
export function selectedCandidate(track: Track) {
  return track.candidates.find((candidate) => candidate.id === track.selected_candidate_id);
}

const filterTests: Record<QueueFilter, (track: Track) => boolean> = {
  all: () => true,
  review: isReview,
  problems: isProblem,
  ready: isReady,
};
export function matchesFilter(track: Track, filter: QueueFilter) {
  return filterTests[filter](track);
}

export function statusFor(track: Track): { label: string; tone: string; icon: IconName } {
  const error = (label: string) => ({ label, tone: "error", icon: "alert" as const });
  if (track.status === "corrupt") return error("Damaged");
  if (track.stage === "skipped") return { label: "Skipped", tone: "muted", icon: "skip" };
  if (track.is_missing) return error("File missing");
  if (track.stage === "failed" || track.status === "failed" || track.status === "provider_error")
    return error("Failed");
  if (track.stage === "review")
    return {
      label:
        selectedCandidate(track) || track.candidates.length > 1
          ? "Needs review"
          : track.candidates.length
            ? "Uncertain"
            : "Not identified",
      tone: "review",
      icon: "info",
    };
  if (isReady(track)) return { label: "Ready", tone: "success", icon: "check" };
  return { label: "Processing", tone: "processing", icon: "waveform" };
}

export function problemTitle(track: Track) {
  if (track.status === "corrupt") return "Damaged audio file";
  if (track.is_missing) return "Source file is missing";
  return "Could not process this track";
}

export function friendlyTrackError(track: Track) {
  if (!track.error) return "";
  if (track.status === "corrupt")
    return "The audio stream could not be decoded safely. This file will not be written.";
  if (track.is_missing) return "The source file is no longer available at its original location.";
  return "magpie-fi could not finish this track. Open technical details if you need the diagnostic message.";
}

function queuePriority(track: Track) {
  if (isReview(track)) return 0;
  if (isProblem(track)) return 1;
  if (isReady(track)) return 2;
  return 3;
}

export function compareTracks(first: Track, second: Track, order: QueueOrder) {
  if (order === "status")
    return (
      queuePriority(first) - queuePriority(second) ||
      compareText(trackTitle(first), trackTitle(second))
    );
  if (order === "artist")
    return (
      compareText(trackArtist(first), trackArtist(second)) ||
      compareText(trackTitle(first), trackTitle(second))
    );
  return compareText(trackTitle(first), trackTitle(second));
}

function trackTitle(track: Track) {
  const candidate = selectedCandidate(track) ?? track.candidates[0];
  return candidate?.title || track.current_title || fileStem(track.filename);
}
function trackArtist(track: Track) {
  const candidate = selectedCandidate(track) ?? track.candidates[0];
  return candidate?.artist || track.current_artist || "Unknown artist";
}
function compareText(first: string, second: string) {
  return first.localeCompare(second, undefined, { numeric: true, sensitivity: "base" });
}

export function preferredTrack(tracks: Track[]) {
  return [...tracks].sort(
    (first, second) =>
      queuePriority(first) - queuePriority(second) || first.filename.localeCompare(second.filename),
  )[0];
}

export function filterTitle(filter: QueueFilter) {
  return {
    all: "Music queue",
    review: "Needs review",
    problems: "Issues",
    ready: "Ready to write",
  }[filter];
}
export function folderName(path: string) {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? "";
}
export function fileStem(filename: string) {
  return filename.replace(/\.[^/.]+$/, "");
}
export function outputFilename(track: Track, candidate: Candidate) {
  const extension = track.filename.includes(".")
    ? `.${track.filename.split(".").pop()?.toLowerCase()}`
    : "";
  return `${safeName(candidate.artist || "Unknown Artist")} - ${safeName(candidate.title || "Unknown Title")}${extension}`;
}
function safeName(value: string) {
  return value
    .replace(/[\\/:*?"<>|]/g, " ")
    .replace(/\s+/g, " ")
    .replace(/^[ .]+|[ .]+$/g, "");
}
export function friendlyError(value: string) {
  if (/fetch|network|load failed/i.test(value))
    return "The local service is not responding. Make sure ./dev.sh is running, then reconnect.";
  if (/permission|denied/i.test(value))
    return `${value} Check that magpie-fi can read the inbox and write to the library folder.`;
  return value;
}

export function googleMetadataUrl(candidate: Candidate) {
  const artist = String(candidate.artist || "").replaceAll('"', "");
  const title = String(candidate.title || "").replaceAll('"', "");
  return `https://www.google.com/search?q=${encodeURIComponent(`"${artist}" "${title}" song album or single album name original release year`)}`;
}

export function candidateSources(candidate: Candidate) {
  const { sources } = evidenceOf(candidate);
  const names = Array.isArray(sources)
    ? sources.filter((source): source is string => typeof source === "string").map(providerName)
    : [];
  return names.length
    ? [...new Set(names)].join(" + ")
    : providerName(candidate.provider || "catalog");
}

export function providerName(source: string) {
  const names: Record<string, string> = {
    acoustid: "AcoustID",
    audiomack: "Audiomack",
    deezer: "Deezer",
    discogs: "Discogs",
    itunes: "Apple Music",
    lastfm: "Last.fm",
    genius: "Genius",
    musicbrainz: "MusicBrainz",
    navahang: "Navahang",
    radiojavan: "Radio Javan",
    shazam: "Shazam",
    songrec: "SongRec / Shazam",
    soundcloud: "SoundCloud",
    spotify: "Spotify",
    youtube: "YouTube",
    ffmpeg: "FFmpeg",
    manual: "Manual entry",
    catalog: "Catalog",
  };
  return names[source] || source;
}

export function metadataAudit(candidate: Candidate) {
  const stored = evidenceOf(candidate).metadata_completion;
  if (typeof stored?.score === "number" && Array.isArray(stored.missing_fields))
    return {
      score: stored.score,
      coreComplete: stored.core_complete === true,
      missing: stored.missing_fields as string[],
    };
  const fields: Array<[string, boolean, number]> = [
    ["title", Boolean(candidate.title?.trim()), 18],
    ["artist", Boolean(candidate.artist?.trim()), 18],
    ["album", Boolean(candidate.album?.trim()), 16],
    ["cover", Boolean(candidate.cover_url?.trim()), 16],
    ["year", Boolean(candidate.year?.trim() || candidate.release_date?.trim()), 10],
    ["genre", Boolean(candidate.genre?.trim()), 8],
    ["track", Boolean(candidate.track_number), 6],
    ["album artist", Boolean(candidate.album_artist?.trim()), 4],
    ["ISRC", Boolean(candidate.isrc?.trim()), 4],
  ];
  return {
    score: fields
      .filter(([, present]) => present)
      .reduce((total, [, , weight]) => total + weight, 0),
    coreComplete: fields.slice(0, 4).every(([, present]) => present),
    missing: fields.filter(([, present]) => !present).map(([name]) => name),
  };
}

/** Filing stopped because the library probably has this song already. */
export const isPossibleDuplicate = (track: Track) =>
  track.stage === "review" &&
  (track.stage_message ?? "").startsWith("Possible duplicate: you may already have this song as ");
