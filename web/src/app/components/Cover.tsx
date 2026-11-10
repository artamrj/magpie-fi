import { useState } from "react";
import type { Candidate, Track } from "../../api/types";
import { Icon } from "../Icons";

type Size = "small" | "medium" | "large";

/** A cover served by magpie-fi; a music-note placeholder when there is none. */
function Cover({ src, alt, size }: { src?: string; alt: string; size: Size }) {
  const [failed, setFailed] = useState<string>();
  if (!src || failed === src)
    return (
      <span className={`artwork artwork-${size} artwork-missing`}>
        <Icon name="music" size={size === "large" ? 28 : 18} />
      </span>
    );
  return (
    <img
      className={`artwork artwork-${size}`}
      src={src}
      alt={alt}
      loading="lazy"
      onError={() => setFailed(src)}
    />
  );
}

/** The cover that will be written for the track's chosen match. */
export function ChosenCover({
  track,
  candidate,
  size,
}: {
  track: Track;
  candidate: Candidate;
  size: Size;
}) {
  const version = encodeURIComponent(candidate.cover_url || candidate.id);
  return (
    <Cover
      src={`/api/tracks/${track.id}/artwork/preview?v=${version}`}
      alt={`Cover for ${candidate.album || candidate.title || "track"}`}
      size={size}
    />
  );
}

/** The catalog cover of a suggested match. */
export function CandidateCover({ candidate, size }: { candidate: Candidate; size: Size }) {
  return (
    <Cover
      src={
        candidate.cover_url
          ? `/api/candidates/${candidate.id}/artwork/preview?v=${encodeURIComponent(candidate.cover_url)}`
          : undefined
      }
      alt={`Cover for ${candidate.album || candidate.title || "track"}`}
      size={size}
    />
  );
}

/** The cover embedded in the original file. */
export function OriginalCover({ track, size }: { track: Track; size: Size }) {
  return (
    <Cover
      src={`/api/tracks/${track.id}/artwork/original?v=${encodeURIComponent(track.filename)}`}
      alt={`Embedded cover from ${track.filename}`}
      size={size}
    />
  );
}

/** The cover inside a library song (Files tab). */
export function LibraryCover({ path, size }: { path: string; size: Size }) {
  return (
    <Cover
      src={`/api/files/artwork?path=${encodeURIComponent(path)}`}
      alt="Cover inside the song"
      size={size}
    />
  );
}
