import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { post } from "../../api/client";
import {
  isActiveRequest,
  queryKeys,
  useRequestSongsQuery,
  useRequestsQuery,
} from "../../api/queries";
import type { MusicRequest, WantedSong } from "../../api/types";
import { Icon, type IconName } from "../Icons";
import { haptic, telegram } from "../telegram";
import { EmptyState, ListSkeleton } from "./EmptyStates";
import type { Feedback } from "./ReviewList";
import { BottomActions, Cell, Field, PageTitle, Section, Spinner, Tag } from "./ui";

const DAY = 24 * 60 * 60 * 1000;
const RECENT_LIMIT = 12;

/**
 * Paste a link, then see what became of what you sent: what is on the way right now, and what
 * settled in the last day. Like a browser's download list, each row is one thing you asked for,
 * says in one word where it stands, and leads to the result; it is not a log.
 */
export function AddMusic({
  wide,
  feedback,
  onOpenTrack,
}: {
  wide: boolean;
  feedback: Feedback;
  onOpenTrack: (trackId: number) => void;
}) {
  const queryClient = useQueryClient();
  const [url, setUrl] = useState("");
  const [sending, setSending] = useState(false);
  const [openId, setOpenId] = useState<number>();
  const [showEarlier, setShowEarlier] = useState(false);
  const requests = useRequestsQuery();
  const all = (requests.data ?? []).filter((request) => !isRemoved(request));

  const submit = async (link = url.trim()) => {
    if (!link) return;
    setSending(true);
    try {
      const result = await post<{ id: number; again: boolean; queued: number }>("/requests", {
        url: link,
      });
      setUrl("");
      haptic.success();
      if (result.again)
        feedback.notify(
          result.queued ? "Already on the list; trying it again" : "Already on the list",
        );
      await queryClient.invalidateQueries({ queryKey: queryKeys.requests });
    } catch (reason) {
      haptic.error();
      feedback.fail((reason as Error).message);
    } finally {
      setSending(false);
    }
  };

  const paste = async () => {
    try {
      const text = await navigator.clipboard.readText();
      if (text) setUrl(text.trim());
    } catch {
      document.getElementById("music-link")?.focus();
    }
  };

  const retry = async (request: MusicRequest) => {
    try {
      if (request.status === "failed") {
        // The link itself could not be read: ask for it afresh.
        await submit(request.url);
        return;
      }
      await post(`/requests/${request.id}/retry`);
      await queryClient.invalidateQueries({ queryKey: queryKeys.requests });
      feedback.notify("Trying again");
    } catch (reason) {
      feedback.fail((reason as Error).message);
    }
  };

  // The clock stands still between refetches; the list refetches every few seconds anyway.
  const now = requests.dataUpdatedAt || 0;
  const active = all.filter(isActiveRequest);
  const settled = all.filter((request) => !isActiveRequest(request));
  const recent = settled.filter((request) => now - Date.parse(request.updated_at) < DAY);
  const earlier = settled.filter((request) => !recent.includes(request));
  const shownRecent = recent.slice(0, RECENT_LIMIT);
  const hidden = [...recent.slice(RECENT_LIMIT), ...earlier];
  const open = all.find((request) => request.id === openId);

  const form = (
    <>
      <PageTitle
        title="Add music"
        subtitle={
          telegram
            ? "Paste a link, or send a link or a file to the bot."
            : "Paste a link, or send a link or a file to the bot in Telegram."
        }
      />
      <form
        className="section"
        onSubmit={(event) => {
          event.preventDefault();
          void submit();
        }}
      >
        <Field
          icon="link"
          id="music-link"
          value={url}
          onChange={(event) => setUrl(event.target.value)}
          placeholder="Spotify, Apple Music, YouTube, Deezer, Shazam…"
          inputMode="url"
          autoComplete="off"
          enterKeyHint="go"
          action={
            url ? (
              <button
                type="button"
                className="small-button"
                aria-label="Clear"
                onClick={() => setUrl("")}
              >
                <Icon name="x" size={16} />
              </button>
            ) : (
              <button type="button" className="small-button" onClick={() => void paste()}>
                Paste
              </button>
            )
          }
        />
        <p className="section-footer">A song, an album or a whole playlist.</p>
      </form>
    </>
  );

  const row = (request: MusicRequest) => (
    <RequestRow
      key={request.id}
      request={request}
      now={now}
      open={open?.id === request.id}
      wide={wide}
      onToggle={() => setOpenId(open?.id === request.id && !wide ? undefined : request.id)}
      onRetry={() => void retry(request)}
      onOpenTrack={onOpenTrack}
    />
  );

  const feed = requests.isPending ? (
    <section className="section">
      <ListSkeleton rows={3} />
    </section>
  ) : all.length === 0 ? (
    <EmptyState
      icon="music"
      title="Nothing sent yet"
      description="What you paste here or send to the bot shows up here until it is in your library."
    />
  ) : (
    <>
      {active.length > 0 && <Section title="On the way">{active.map(row)}</Section>}
      {shownRecent.length > 0 && (
        <Section
          title="Recent"
          footer="Songs stay here for a day. Everything that was added is in your library."
        >
          {shownRecent.map(row)}
        </Section>
      )}
      {hidden.length > 0 && (
        <Section title={showEarlier ? "Earlier" : undefined}>
          {showEarlier ? (
            hidden.map(row)
          ) : (
            <Cell
              tone="link"
              title={`Show ${hidden.length} earlier`}
              onClick={() => setShowEarlier(true)}
            />
          )}
        </Section>
      )}
    </>
  );

  const actions = (
    <BottomActions
      primary={
        url.trim() ? { text: "Get it", onClick: () => void submit(), busy: sending } : undefined
      }
    />
  );

  if (wide)
    return (
      <>
        <section className="pane">
          <div className="screen">
            {form}
            {feed}
          </div>
          {actions}
        </section>
        <aside className="detail">
          <div className="screen">
            {open && open.total > 1 ? (
              <>
                <PageTitle title={requestName(open)} subtitle={summaryLine(open)} />
                <RequestSongs request={open} onOpenTrack={onOpenTrack} />
              </>
            ) : (
              <EmptyState
                icon="link"
                title="Albums and playlists open here"
                description="Pick one on the left to see each of its songs."
              />
            )}
          </div>
        </aside>
      </>
    );

  return (
    <main className="screen">
      {form}
      {feed}
      {actions}
    </main>
  );
}

/** Removed by the owner, nothing else to say: the row is gone, like a cleared download. */
function isRemoved(request: MusicRequest) {
  return request.total > 0 && request.removed === request.total;
}

/** "Artist – Title" from the request, without the artist repeated inside the title. */
function requestName(request: MusicRequest) {
  if (request.kind === "upload" && request.total > 1) return `${request.total} files from Telegram`;
  const raw = request.title?.trim();
  if (!raw) return request.url ? linkName(request.url) : "File from Telegram";
  const [artist, ...rest] = raw.split(" – ");
  if (rest.length === 0) return raw;
  let title = rest.join(" – ");
  const prefix = `${artist.toLowerCase()} - `;
  if (title.toLowerCase().startsWith(prefix)) title = title.slice(prefix.length);
  return `${title} · ${artist}`;
}

function linkName(url: string) {
  try {
    const parsed = new URL(url);
    return `${parsed.hostname.replace(/^www\./, "")}${parsed.pathname}`.slice(0, 60);
  } catch {
    return url.slice(0, 60);
  }
}

function sourceName(request: MusicRequest) {
  if (request.kind === "upload") return "Telegram";
  const host = request.url.toLowerCase();
  if (host.includes("spotify")) return "Spotify";
  if (host.includes("apple")) return "Apple Music";
  if (host.includes("youtu")) return "YouTube";
  if (host.includes("deezer")) return "Deezer";
  if (host.includes("shazam")) return "Shazam";
  if (host.includes("soundcloud")) return "SoundCloud";
  return "Link";
}

type State = "reading" | "working" | "pick" | "added" | "failed" | "mixed";

function stateOf(request: MusicRequest): State {
  if (request.status === "reading") return "reading";
  if (request.status === "failed") return "failed";
  if (request.working > 0) return "working";
  if (request.total > 1) return request.done === request.total ? "added" : "mixed";
  if (request.review > 0) return "pick";
  if (request.failed > 0) return "failed";
  return "added";
}

/** The one line under a request's name. */
function summaryLine(request: MusicRequest) {
  const state = stateOf(request);
  if (state === "reading") return "Reading the link…";
  if (request.status === "failed")
    return `Couldn’t read the link: ${request.error || "unknown error"}`;
  if (request.total <= 1) {
    if (state === "working") return "On the way";
    if (state === "pick") return "Needs your pick";
    if (state === "failed") return `Failed: ${request.first_error || "unknown error"}`;
    return "Added";
  }
  const parts = [`${request.done} of ${request.total} added`];
  if (request.working) parts.push(`${request.working} on the way`);
  if (request.review) parts.push(`${request.review} need your pick`);
  if (request.failed) parts.push(`${request.failed} failed`);
  if (request.removed) parts.push(`${request.removed} removed`);
  return parts.join(" · ");
}

function RequestRow({
  request,
  now,
  open,
  wide,
  onToggle,
  onRetry,
  onOpenTrack,
}: {
  request: MusicRequest;
  now: number;
  open: boolean;
  wide: boolean;
  onToggle: () => void;
  onRetry: () => void;
  onOpenTrack: (trackId: number) => void;
}) {
  const state = stateOf(request);
  const multi = request.total > 1;
  const when = relativeTime(request.updated_at, now);
  const lead = (
    <span className="artwork artwork-small">
      <Icon name={request.kind === "upload" ? "music" : "link"} size={18} />
    </span>
  );
  const trailing =
    state === "reading" || state === "working" ? (
      <Spinner />
    ) : state === "failed" ? (
      <button type="button" className="small-button" onClick={onRetry}>
        Retry
      </button>
    ) : state === "pick" ? (
      <Tag tone="warn">Pick</Tag>
    ) : state === "added" ? (
      <Tag tone="good">
        <Icon name="check" size={12} />
      </Tag>
    ) : undefined;
  // What matters first: where it stands, then where it came from, then when.
  const subtitle = [summaryLine(request), sourceName(request), when].filter(Boolean).join(" · ");
  // What a tap does: a song waiting for you opens in Review; a collection opens its songs.
  const onClick = multi
    ? onToggle
    : state === "pick" && request.track_id
      ? () => {
          haptic.tap();
          onOpenTrack(request.track_id!);
        }
      : undefined;
  return (
    <div>
      <Cell
        lead={lead}
        title={requestName(request)}
        subtitle={subtitle}
        trailing={trailing}
        chevron={Boolean(onClick) && !(multi && open && !wide)}
        selected={wide && open}
        onClick={onClick}
      />
      {multi && open && !wide && <RequestSongs request={request} onOpenTrack={onOpenTrack} />}
    </div>
  );
}

const songStates: Record<
  string,
  { label: string; tone: "good" | "warn" | "bad" | "info" | "gray" }
> = {
  pending: { label: "Waiting", tone: "gray" },
  downloading: { label: "Downloading", tone: "info" },
  identifying: { label: "Identifying", tone: "info" },
  filed: { label: "Added", tone: "good" },
  in_library: { label: "Already there", tone: "good" },
  review: { label: "Pick", tone: "warn" },
  failed: { label: "Failed", tone: "bad" },
  removed: { label: "Removed", tone: "gray" },
};

/** Every song of an album, playlist or batch of files, each leading to its own result. */
function RequestSongs({
  request,
  onOpenTrack,
}: {
  request: MusicRequest;
  onOpenTrack: (trackId: number) => void;
}) {
  const songs = useRequestSongsQuery(request);
  const icon: IconName = request.kind === "upload" ? "music" : "link";
  return (
    <Section
      footer={
        request.url ? (
          <a href={request.url} target="_blank" rel="noreferrer">
            Open the original link
          </a>
        ) : undefined
      }
    >
      {songs.isPending && <Cell title="Loading…" lead={<Spinner />} />}
      {(songs.data ?? []).map((song) => (
        <SongRow key={song.position} song={song} icon={icon} onOpenTrack={onOpenTrack} />
      ))}
    </Section>
  );
}

function SongRow({
  song,
  icon,
  onOpenTrack,
}: {
  song: WantedSong;
  icon: IconName;
  onOpenTrack: (trackId: number) => void;
}) {
  const state = songStates[song.status] ?? { label: song.status, tone: "gray" as const };
  const busy =
    song.status === "downloading" || song.status === "identifying" || song.status === "pending";
  const canOpen = song.status === "review" && Boolean(song.track_id);
  return (
    <Cell
      lead={
        <span className="artwork artwork-small">
          <Icon name={icon} size={18} />
        </span>
      }
      title={song.title}
      subtitle={[song.artist, song.status === "failed" ? song.error : ""]
        .filter(Boolean)
        .join(" · ")}
      trailing={busy ? <Spinner /> : <Tag tone={state.tone}>{state.label}</Tag>}
      chevron={canOpen}
      onClick={canOpen ? () => onOpenTrack(song.track_id!) : undefined}
    />
  );
}

function relativeTime(iso: string, now: number) {
  const then = Date.parse(iso);
  if (Number.isNaN(then)) return "";
  const minutes = Math.round((now - then) / 60000);
  if (minutes < 1) return "now";
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours} h`;
  const days = Math.round(hours / 24);
  if (days === 1) return "yesterday";
  if (days < 7) return `${days} days`;
  return new Date(then).toLocaleDateString(undefined, { day: "numeric", month: "short" });
}
