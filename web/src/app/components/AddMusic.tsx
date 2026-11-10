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
import { haptic, telegram } from "../telegram";
import { EmptyState } from "./EmptyStates";
import type { Feedback } from "./ReviewList";
import { BottomActions, Cell, Field, PageTitle, Section, Spinner, Tag } from "./ui";

const songStatus: Record<
  string,
  { label: string; tone: "good" | "warn" | "bad" | "info" | "gray" }
> = {
  pending: { label: "Waiting", tone: "gray" },
  downloading: { label: "Downloading", tone: "info" },
  identifying: { label: "Identifying", tone: "info" },
  filed: { label: "Added", tone: "good" },
  in_library: { label: "Already there", tone: "good" },
  review: { label: "Your pick", tone: "warn" },
  failed: { label: "Failed", tone: "bad" },
};

/** Paste a link; follow every request until its songs are in the library. */
export function AddMusic({
  wide,
  feedback,
  onReview,
}: {
  wide: boolean;
  feedback: Feedback;
  onReview: () => void;
}) {
  const queryClient = useQueryClient();
  const [url, setUrl] = useState("");
  const [sending, setSending] = useState(false);
  const [openId, setOpenId] = useState<number>();
  const requests = useRequestsQuery();
  const recent = useRecentSongsQuery();
  const list = requests.data ?? [];
  const open = list.find((request) => request.id === openId) ?? (wide ? list[0] : undefined);

  const submit = async () => {
    const link = url.trim();
    if (!link) return;
    setSending(true);
    try {
      const created = await post<{ id: number }>("/requests", { url: link });
      setUrl("");
      setOpenId(created.id);
      haptic.success();
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
      await post(`/requests/${request.id}/retry`);
      await queryClient.invalidateQueries({ queryKey: queryKeys.requests });
      feedback.notify(`Trying ${request.failed} ${request.failed === 1 ? "song" : "songs"} again`);
    } catch (reason) {
      feedback.fail((reason as Error).message);
    }
  };

  const active = list.filter(
    (request) => isActiveRequest(request) || request.review || request.failed,
  );
  const finished = list.filter((request) => !active.includes(request));

  const form = (
    <>
      <PageTitle
        title="Add music"
        subtitle={
          telegram
            ? "Paste a link, or send one to the bot in chat."
            : "Paste a link, or send one to the bot in Telegram."
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
        <p className="section-footer">
          A song, an album or a whole playlist. Files sent to the bot show up here too.
        </p>
      </form>
    </>
  );

  const requestRow = (request: MusicRequest) => {
    const title =
      request.kind === "upload" && request.total > 1
        ? `${request.total} files from Telegram`
        : request.title || (request.kind === "upload" ? "File from Telegram" : request.url);
    const kind =
      request.kind === "upload"
        ? "Telegram"
        : request.kind[0].toUpperCase() + request.kind.slice(1);
    const summary =
      request.status === "reading"
        ? "Reading the link…"
        : request.status === "failed"
          ? request.error || "Failed"
          : request.total === 1
            ? request.done
              ? "In your library"
              : request.review
                ? "Needs your pick"
                : request.failed
                  ? request.error || "Failed"
                  : "On the way…"
            : `${request.done} of ${request.total} in your library`;
    const isOpen = open?.id === request.id;
    const multi = request.total > 1 && request.status !== "failed";
    return (
      <div key={request.id}>
        <Cell
          lead={
            <span className="artwork artwork-small">
              <Icon name={request.kind === "upload" ? "music" : "link"} size={18} />
            </span>
          }
          title={title}
          subtitle={`${kind} · ${summary}`}
          trailing={
            isActiveRequest(request) ? (
              <Spinner />
            ) : request.review ? (
              <Tag tone="warn">{request.review}</Tag>
            ) : request.failed ? (
              <Tag tone="bad">{request.failed}</Tag>
            ) : request.done === request.total && request.total > 0 ? (
              <Tag tone="good">
                <Icon name="check" size={12} />
              </Tag>
            ) : undefined
          }
          chevron
          selected={wide && isOpen}
          onClick={() => setOpenId(isOpen && !wide ? undefined : request.id)}
        />
        {multi && (
          <>
            <div className="stacked-bar" aria-hidden="true">
              <i className="done" style={{ width: `${(request.done / request.total) * 100}%` }} />
              <i
                className="work"
                style={{ width: `${(request.working / request.total) * 100}%` }}
              />
              <i className="rev" style={{ width: `${(request.review / request.total) * 100}%` }} />
              <i className="fail" style={{ width: `${(request.failed / request.total) * 100}%` }} />
            </div>
            <div className="legend">
              <span>
                <i style={{ background: "var(--good)" }} />
                {request.done} done
              </span>
              {request.working > 0 && (
                <span>
                  <i style={{ background: "var(--button)" }} />
                  {request.working} on the way
                </span>
              )}
              {request.review > 0 && (
                <span>
                  <i style={{ background: "var(--warn)" }} />
                  {request.review} need your pick
                </span>
              )}
              {request.failed > 0 && (
                <span>
                  <i style={{ background: "var(--bad)" }} />
                  {request.failed} failed
                </span>
              )}
            </div>
          </>
        )}
        {isOpen && !wide && <RequestSongs request={request} onRetry={retry} onReview={onReview} />}
      </div>
    );
  };

  const lists = (
    <>
      {list.length === 0 && !requests.isPending ? (
        <EmptyState
          icon="music"
          title="Nothing requested yet"
          description="Songs you ask for appear here until they are in your library."
        />
      ) : (
        <>
          {active.length > 0 && <Section title="In progress">{active.map(requestRow)}</Section>}
          {finished.length > 0 && (
            <Section title="Done">{finished.slice(0, 10).map(requestRow)}</Section>
          )}
        </>
      )}
      {(recent.data?.length ?? 0) > 0 && (
        <Section title="Added recently">
          {recent.data!.slice(0, 8).map((song, index) => (
            <Cell
              key={`${song.added_at}-${index}`}
              lead={
                <span className="artwork artwork-small">
                  <Icon name="music" size={18} />
                </span>
              }
              title={song.title ?? "Unknown title"}
              subtitle={[song.artist, song.album].filter(Boolean).join(" · ")}
              trailing={<span>{relativeDay(song.added_at)}</span>}
            />
          ))}
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
            {lists}
          </div>
          {actions}
        </section>
        <aside className="detail">
          <div className="screen">
            {open ? (
              <>
                <PageTitle title={open.title || open.url} />
                <RequestSongs request={open} onRetry={retry} onReview={onReview} />
              </>
            ) : (
              <EmptyState
                icon="link"
                title="Pick a request"
                description="Its songs and their progress show here."
              />
            )}
          </div>
        </aside>
      </>
    );

  return (
    <main className="screen">
      {form}
      {lists}
      {actions}
    </main>
  );
}

function RequestSongs({
  request,
  onRetry,
  onReview,
}: {
  request: MusicRequest;
  onRetry: (request: MusicRequest) => Promise<void>;
  onReview: () => void;
}) {
  const songs = useRequestSongsQuery(request);
  return (
    <Section
      title={request.total > 1 ? `${request.total} songs` : undefined}
      footer={
        request.url ? (
          <a href={request.url} target="_blank" rel="noreferrer">
            Open the original link
          </a>
        ) : undefined
      }
    >
      {request.failed > 0 && (
        <Cell
          tone="accent"
          lead={<Icon name="refresh" />}
          title={`Try ${request.failed} failed again`}
          onClick={() => void onRetry(request)}
        />
      )}
      {request.review > 0 && (
        <Cell
          tone="accent"
          lead={<Icon name="inbox" />}
          title={`${request.review} ${request.review === 1 ? "needs" : "need"} your pick`}
          chevron
          onClick={onReview}
        />
      )}
      {songs.isPending && <Cell title="Loading…" lead={<Spinner />} />}
      {(songs.data ?? []).map((song) => {
        const status = songStatus[song.status] ?? { label: song.status, tone: "gray" as const };
        return (
          <Cell
            key={song.position}
            title={song.title}
            subtitle={[song.artist, song.status === "failed" ? song.error : ""]
              .filter(Boolean)
              .join(" · ")}
            trailing={<Tag tone={status.tone}>{status.label}</Tag>}
          />
        );
      })}
    </Section>
  );
}

function relativeDay(iso: string) {
  const date = new Date(iso);
  const today = new Date();
  const sameDay = date.toDateString() === today.toDateString();
  if (sameDay) return date.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
  const yesterday = new Date(today);
  yesterday.setDate(today.getDate() - 1);
  if (date.toDateString() === yesterday.toDateString()) return "Yesterday";
  return date.toLocaleDateString(undefined, { day: "numeric", month: "short" });
}
