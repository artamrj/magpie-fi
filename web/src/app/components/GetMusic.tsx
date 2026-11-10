import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "@/api/client";
import type { MusicRequest, WantedSong } from "@/api/types";
import { Icon } from "../Icons";

const statusLabels: Record<string, string> = {
  pending: "Waiting",
  downloading: "Downloading",
  identifying: "Identifying",
  filed: "Added",
  in_library: "Already in library",
  review: "Needs review",
  failed: "Failed",
};

const isActive = (request: MusicRequest) => request.status === "reading" || request.working > 0;

/** Paste a song, album or playlist link; follow every request until it is in the library. */
export function GetMusic({ onError }: { onError: (message: string) => void }) {
  const queryClient = useQueryClient();
  const [url, setUrl] = useState("");
  const [sending, setSending] = useState(false);
  const [open, setOpen] = useState<number>();
  const requests = useQuery({
    queryKey: ["requests"],
    queryFn: () => api<MusicRequest[]>("/requests"),
    refetchInterval: (query) => (query.state.data?.some(isActive) ? 2000 : 15000),
    refetchIntervalInBackground: true,
  });

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!url.trim()) return;
    setSending(true);
    try {
      const created = await api<{ id: number }>("/requests", {
        method: "POST",
        body: JSON.stringify({ url: url.trim() }),
      });
      setUrl("");
      setOpen(created.id);
      await queryClient.invalidateQueries({ queryKey: ["requests"] });
    } catch (reason) {
      onError((reason as Error).message);
    } finally {
      setSending(false);
    }
  };

  const retry = async (id: number) => {
    try {
      await api(`/requests/${id}/retry`, { method: "POST", body: "{}" });
      await queryClient.invalidateQueries({ queryKey: ["requests"] });
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

      {list.length === 0 ? (
        <div className="empty-state">
          <span>
            <Icon name="music" size={28} />
          </span>
          <h2>Nothing requested yet</h2>
          <p>Songs you ask for appear here until they are in your library.</p>
        </div>
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
  // Songs and uploaded files are named after their song; albums and playlists have a title.
  const title =
    request.title ??
    (request.kind === "album" || request.kind === "playlist" ? request.url : undefined);
  return (
    <li className={`request-card ${request.status}`}>
      <button className="request-summary" onClick={onToggle} aria-expanded={open}>
        <span className="request-kind">
          {request.kind === "upload" ? "Telegram" : request.kind}
        </span>
        <span className="request-title">
          <b>
            {title ??
              (request.kind === "upload" && request.total > 1 ? (
                `${request.total} files`
              ) : (
                <SongTitle requestId={request.id} />
              ))}
          </b>
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
        {isActive(request) && <span className="spinner" />}
        <Icon name="chevron" size={16} />
      </button>
      {request.status !== "failed" && request.total > 0 && (
        <progress max="100" value={percent} aria-label={`${percent}% in library`} />
      )}
      {open && <RequestSongs request={request} onRetry={onRetry} />}
    </li>
  );
}

/** A song request has no title of its own: show its song. */
function SongTitle({ requestId }: { requestId: number }) {
  const songs = useSongs(requestId, false);
  const song = songs.data?.[0];
  return <>{song ? [song.artist, song.title].filter(Boolean).join(" – ") : "Song"}</>;
}

function useSongs(requestId: number, live: boolean) {
  return useQuery({
    queryKey: ["requests", requestId],
    queryFn: () => api<WantedSong[]>(`/requests/${requestId}`),
    refetchInterval: live ? 3000 : false,
  });
}

function RequestSongs({ request, onRetry }: { request: MusicRequest; onRetry: () => void }) {
  const songs = useSongs(request.id, isActive(request));
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
