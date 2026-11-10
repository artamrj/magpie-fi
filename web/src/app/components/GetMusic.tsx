import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { post } from "../../api/client";
import {
  isActiveRequest,
  queryKeys,
  useRecentSongsQuery,
  useRequestSongsQuery,
  useRequestsQuery,
} from "../../api/queries";
import type { MusicRequest } from "../../api/types";
import { Icon } from "../Icons";
import { EmptyState } from "./EmptyStates";

const statusLabels: Record<string, string> = {
  pending: "Waiting",
  downloading: "Downloading",
  identifying: "Identifying",
  filed: "Added",
  in_library: "Already in library",
  review: "Needs review",
  failed: "Failed",
};

/** Paste a song, album or playlist link; follow every request until it is in the library. */
export function GetMusic({ onError }: { onError: (message: string) => void }) {
  const queryClient = useQueryClient();
  const [url, setUrl] = useState("");
  const [sending, setSending] = useState(false);
  const [open, setOpen] = useState<number>();
  const requests = useRequestsQuery();

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!url.trim()) return;
    setSending(true);
    try {
      const created = await post<{ id: number }>("/requests", { url: url.trim() });
      setUrl("");
      setOpen(created.id);
      await queryClient.invalidateQueries({ queryKey: queryKeys.requests });
    } catch (reason) {
      onError((reason as Error).message);
    } finally {
      setSending(false);
    }
  };

  const retry = async (id: number) => {
    try {
      await post(`/requests/${id}/retry`);
      await queryClient.invalidateQueries({ queryKey: queryKeys.requests });
    } catch (reason) {
      onError((reason as Error).message);
    }
  };

  const list = requests.data ?? [];
  return (
    <main className="get-music" id="workspace">
      <form className="get-music-form" onSubmit={submit}>
        <label htmlFor="music-link">
          <b>Get music</b>
          <small>
            Paste a song, album or playlist link from Spotify, Apple Music, Deezer, YouTube, YouTube
            Music or Shazam.
          </small>
        </label>
        <div className="get-music-input">
          <input
            id="music-link"
            value={url}
            onChange={(event) => setUrl(event.target.value)}
            placeholder="https://open.spotify.com/playlist/…"
            autoComplete="off"
            inputMode="url"
          />
          <button className="primary-action" disabled={sending || !url.trim()}>
            {sending ? <span className="spinner" /> : <Icon name="sparkles" />}
            Get it
          </button>
        </div>
      </form>

      <RecentlyAdded />

      {list.length === 0 ? (
        <EmptyState
          icon="music"
          title="Nothing requested yet"
          description="Songs you ask for appear here until they are in your library."
        />
      ) : (
        <ul className="request-list">
          {list.map((request) => (
            <RequestCard
              key={request.id}
              request={request}
              open={open === request.id}
              onToggle={() => setOpen(open === request.id ? undefined : request.id)}
              onRetry={() => void retry(request.id)}
            />
          ))}
        </ul>
      )}
    </main>
  );
}

function RequestCard({
  request,
  open,
  onToggle,
  onRetry,
}: {
  request: MusicRequest;
  open: boolean;
  onToggle: () => void;
  onRetry: () => void;
}) {
  const percent = request.total ? Math.round((request.done / request.total) * 100) : 0;
  // Several sent files are counted; otherwise the server names a request after its first song.
  const title =
    request.kind === "upload" && request.total > 1
      ? `${request.total} files`
      : (request.title ??
        (request.kind === "album" || request.kind === "playlist" ? request.url : "Song"));
  return (
    <li className={`request-card ${request.status}`}>
      <button className="request-summary" onClick={onToggle} aria-expanded={open}>
        <span className="request-kind">
          {request.kind === "upload" ? "Telegram" : request.kind}
        </span>
        <span className="request-title">
          <b>{title}</b>
          <small>
            {request.status === "reading"
              ? "Reading the link…"
              : request.status === "failed"
                ? request.error
                : [
                    `${request.done} of ${request.total} in library`,
                    request.working ? `${request.working} on the way` : "",
                    request.review ? `${request.review} need review` : "",
                    request.failed ? `${request.failed} failed` : "",
                  ]
                    .filter(Boolean)
                    .join(" · ")}
          </small>
        </span>
        {isActiveRequest(request) && <span className="spinner" />}
        <Icon name="chevron" size={16} />
      </button>
      {request.status !== "failed" && request.total > 0 && (
        <progress max="100" value={percent} aria-label={`${percent}% in library`} />
      )}
      {open && <RequestSongs request={request} onRetry={onRetry} />}
    </li>
  );
}

function RequestSongs({ request, onRetry }: { request: MusicRequest; onRetry: () => void }) {
  const songs = useRequestSongsQuery(request);
  return (
    <div className="request-songs">
      {request.failed > 0 && (
        <button className="compact-button" onClick={onRetry}>
          <Icon name="refresh" size={15} />
          Retry {request.failed} failed
        </button>
      )}
      <ol>
        {(songs.data ?? []).map((song) => (
          <li key={song.position} className={`song-status ${song.status}`}>
            <span>
              <b>{song.title}</b>
              <small>{song.artist}</small>
            </span>
            <em title={song.error ?? undefined}>
              {statusLabels[song.status] ?? song.status}
              {song.status === "failed" && song.error ? ` · ${song.error}` : ""}
            </em>
          </li>
        ))}
      </ol>
      {request.url && (
        <a className="request-source" href={request.url} target="_blank" rel="noreferrer">
          Open the original link
        </a>
      )}
    </div>
  );
}

/** The last songs that went into the library, newest first. */
function RecentlyAdded() {
  const recent = useRecentSongsQuery();
  const songs = recent.data ?? [];
  if (songs.length === 0) return null;
  return (
    <details className="recently-added">
      <summary>
        Recently added <span>{songs.length}</span>
      </summary>
      <ol>
        {songs.map((song, index) => (
          <li key={`${song.added_at}-${index}`} className="song-status filed">
            <span>
              <b>{song.title ?? "Unknown title"}</b>
              <small>{[song.artist, song.album].filter(Boolean).join(" · ")}</small>
            </span>
            <em>{new Date(song.added_at).toLocaleDateString()}</em>
          </li>
        ))}
      </ol>
    </details>
  );
}
