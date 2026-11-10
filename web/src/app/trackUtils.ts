import type { Candidate, Track } from "../api/types";

export const busyPhases = new Set(["scan", "fetch", "apply"]);

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

function queuePriority(track: Track) {
  if (isReview(track)) return 0;
  if (isProblem(track)) return 1;
  if (isReady(track)) return 2;
  return 3;
}

export function preferredTrack(tracks: Track[]) {
  return [...tracks].sort(
    (first, second) =>
      queuePriority(first) - queuePriority(second) || first.filename.localeCompare(second.filename),
  )[0];
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

/** Filing stopped because the library probably has this song already. */
export const isPossibleDuplicate = (track: Track) =>
  track.stage === "review" &&
  (track.stage_message ?? "").startsWith("Possible duplicate: you may already have this song as ");

/** The title and artist a row shows: the accepted match, else the file's own tags, else the file name. */
export function trackTitle(track: Track) {
  return selectedCandidate(track)?.title || track.current_title || fileStem(track.filename);
}
export function trackArtist(track: Track) {
  return selectedCandidate(track)?.artist || track.current_artist || "";
}

/** A chosen match that still needs a cover before the song can be filed. */
export function needsCover(track: Track) {
  const candidate = selectedCandidate(track);
  return Boolean(candidate && candidate.artwork_status !== "verified");
}

/** The one-line reason a song waits in Review. */
export function reviewReason(track: Track) {
  if (isPossibleDuplicate(track)) return "Maybe already in your library";
  if (hasRetryableArtwork(track)) return "Cover download failed";
  if (needsCover(track)) return "Needs a cover";
  const count = track.candidates.length;
  if (count > 1) return `${count} releases match`;
  if (count === 1) return "One match, not sure";
  return "Nothing recognized it";
}

/** The one-line reason a song is in Issues. */
export function problemReason(track: Track) {
  if (track.status === "corrupt") return "Damaged file";
  if (track.is_missing) return "File is missing";
  if (track.stage === "skipped") return "Duplicate upload, skipped";
  if (track.status === "provider_error") return "A source was unavailable";
  return "Could not be processed";
}

/** A review song whose first match is strong enough to accept without a look. */
export function isConfident(track: Track) {
  return (
    isReview(track) &&
    !isProblem(track) &&
    !hasRetryableArtwork(track) &&
    !isPossibleDuplicate(track) &&
    track.candidates.length > 0 &&
    track.candidates[0].score >= 85 &&
    (track.candidates.length === 1 || track.candidates[0].score - track.candidates[1].score >= 8)
  );
}

export type Chip = { text: string; tone: "good" | "warn" | "plain" };

/** What sets a release apart from the file, as short chips. Only differences are shown. */
export function candidateChips(track: Track, candidate: Candidate, rank: number): Chip[] {
  const chips: Chip[] = [];
  const signals = candidateSignals(track, candidate);
  if (rank === 1 && candidate.score >= 85) chips.push({ text: "Best match", tone: "good" });
  else if (candidate.score < 55) chips.push({ text: "Weak match", tone: "warn" });
  const [title, artist, album, duration] = signals;
  if (title.tone === "warning")
    chips.push({ text: titleDifference(track.current_title, candidate.title), tone: "warn" });
  if (artist.tone === "warning") chips.push({ text: "Different artist", tone: "warn" });
  if (album.tone === "warning" && track.current_album)
    chips.push({ text: "Different album", tone: "warn" });
  if (duration.tone === "warning") chips.push({ text: "Length differs", tone: "warn" });
  if (candidate.artwork_status === "cover_required")
    chips.push({ text: "No cover found", tone: "warn" });
  chips.push({ text: candidateSources(candidate), tone: "plain" });
  return chips;
}

function titleDifference(original?: string, proposed?: string) {
  if (!proposed) return "No title";
  const extra = proposed.match(/\((.+?)\)|\[(.+?)\]|- (.+)$/);
  if (
    extra &&
    original &&
    proposed.toLocaleLowerCase().startsWith(original.toLocaleLowerCase().trim())
  )
    return `Title adds “${extra[1] ?? extra[2] ?? extra[3]}”`;
  return "Different title";
}

/** The tag fields a match changes, for "what changes in your file". */
export function changedFields(track: Track, candidate: Candidate) {
  const rows: Array<{ label: string; from?: string; to?: string }> = [
    { label: "Title", from: track.current_title, to: candidate.title },
    { label: "Artist", from: track.current_artist, to: candidate.artist },
    { label: "Album", from: track.current_album, to: candidate.album },
    { label: "Album artist", from: track.current_album_artist, to: candidate.album_artist },
    {
      label: "Track",
      from: track.current_track_number?.toString(),
      to: candidate.track_number
        ? `${candidate.track_number}${candidate.track_total ? ` of ${candidate.track_total}` : ""}`
        : undefined,
    },
    { label: "Year", to: candidate.year || candidate.release_date?.slice(0, 4) },
    { label: "Genre", to: candidate.genre },
  ];
  return rows.filter((row) => row.to && row.to !== row.from);
}
