import { useMemo, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { post } from "../../api/client";
import { queryKeys, useArtistsQuery, useMeQuery, useRecentSongsQuery } from "../../api/queries";
import type { ArtistChanged, ArtistSummary, Setup } from "../../api/types";
import { Icon } from "../Icons";
import { askConfirm, haptic, telegram } from "../telegram";
import { providerName } from "../trackUtils";
import { EmptyState, ListSkeleton } from "./EmptyStates";
import type { Feedback } from "./ReviewList";
import { Cell, Field, PageTitle, Section, Sheet, Spinner, Tag } from "./ui";

const SEPARATORS = /\s+(?:&|x|X|×|و|with|With|vs\.?|feat\.?|ft\.?|featuring)\s+|\s*[,،]\s*/;
const MOVE_WARNING =
  "Their tags are rewritten and they move to the right artist folder. Navidrome may count them as new songs, so play counts can start again.";

/** Everything that is filed: numbers, artists (with fixes for wrong names), and status. */
export function Library({
  setup,
  wide,
  feedback,
}: {
  setup: Setup;
  wide: boolean;
  feedback: Feedback;
}) {
  const artists = useArtistsQuery();
  const recent = useRecentSongsQuery();
  const me = useMeQuery();
  const [search, setSearch] = useState("");
  const [openId, setOpenId] = useState<number>();
  const all = useMemo(() => artists.data ?? [], [artists.data]);
  const shown = useMemo(() => {
    const query = search.trim().toLowerCase();
    return query
      ? all.filter((artist) =>
          [artist.name, ...artist.spellings].some((name) => name.toLowerCase().includes(query)),
        )
      : all;
  }, [all, search]);
  const suspicious = shown.filter((artist) => artist.looks_like_several);
  const open = all.find((artist) => artist.id === openId);
  const sourcesReady = Object.values(setup.sources).filter(Boolean).length;
  const sourcesTotal = Object.keys(setup.sources).length;
  const missingSources = Object.entries(setup.sources)
    .filter(([, ok]) => !ok)
    .map(([name]) => providerName(name));

  const logOut = async () => {
    await post("/logout").catch(() => undefined);
    window.location.reload();
  };

  const artistRow = (artist: ArtistSummary) => (
    <Cell
      key={artist.id}
      lead={<Icon name="user" />}
      title={artist.name}
      subtitle={[
        `${artist.songs} ${artist.songs === 1 ? "song" : "songs"}`,
        artist.spellings.length ? `also written ${artist.spellings.join(", ")}` : "",
      ]
        .filter(Boolean)
        .join(" · ")}
      trailing={artist.looks_like_several ? <Tag tone="warn">Split?</Tag> : undefined}
      chevron
      selected={wide && artist.id === openId}
      onClick={() => {
        haptic.tap();
        setOpenId(artist.id);
      }}
    />
  );

  const content = (
    <>
      <PageTitle title="Library" subtitle="Everything magpie-fi has filed for Navidrome." />
      <div className="stats">
        <div>
          <b>{setup.library_songs.toLocaleString()}</b>
          <small>songs</small>
        </div>
        <div>
          <b>{artists.isPending ? "…" : all.length.toLocaleString()}</b>
          <small>artists</small>
        </div>
        <div>
          <b>{recent.isPending ? "…" : `+${recent.data?.length ?? 0}`}</b>
          <small>added lately</small>
        </div>
      </div>
      <label className="search-field">
        <Icon name="search" size={18} />
        <span className="sr-only">Search artists</span>
        <input
          id="artist-search"
          value={search}
          onChange={(event) => setSearch(event.target.value)}
          placeholder="Search artists"
          autoComplete="off"
        />
      </label>

      {(recent.data?.length ?? 0) > 0 && (
        <Section title="Added recently">
          {recent.data!.slice(0, 6).map((song, index) => (
            <Cell
              key={`${song.added_at}-${index}`}
              lead={
                <span className="artwork artwork-small">
                  <Icon name="music" size={18} />
                </span>
              }
              title={song.title ?? "Unknown title"}
              subtitle={[song.artist, song.album].filter(Boolean).join(" · ")}
              trailing={<span>{addedWhen(song.added_at)}</span>}
            />
          ))}
        </Section>
      )}

      {artists.isPending ? (
        <section className="section">
          <ListSkeleton rows={4} />
        </section>
      ) : all.length === 0 ? (
        <EmptyState
          icon="music"
          title="No artists yet"
          description="Artists appear here once their songs are filed."
        />
      ) : (
        <>
          {suspicious.length > 0 && (
            <Section
              title="Check these"
              footer="Names that look like two artists in one. Open one to split it, or confirm it is a band."
            >
              {suspicious.map(artistRow)}
            </Section>
          )}
          <Section title="Artists">
            {shown.filter((artist) => !artist.looks_like_several).map(artistRow)}
            {shown.length === 0 && (
              <Cell title="No artist matches" subtitle="Try another spelling" />
            )}
          </Section>
        </>
      )}

      <Section title="Status" footer="Settings live in the server’s .env file (MAGPIE_…).">
        <Cell
          lead={
            <Icon
              name="check"
              style={{ color: setup.telegram.configured ? "var(--good)" : "var(--hint)" }}
            />
          }
          title={setup.telegram.configured ? "Telegram bot is on" : "Telegram bot is off"}
          subtitle={
            setup.telegram.configured
              ? `${setup.telegram.owners ?? 0} ${setup.telegram.owners === 1 ? "owner" : "owners"} · ${setup.telegram.guests ?? 0} ${setup.telegram.guests === 1 ? "guest" : "guests"} · files up to 20 MB`
              : "Set MAGPIE_TELEGRAM_BOT_TOKEN and MAGPIE_TELEGRAM_OWNERS"
          }
        />
        <Cell
          lead={
            <Icon
              name={missingSources.length ? "alert" : "check"}
              style={{ color: missingSources.length ? "var(--warn)" : "var(--good)" }}
            />
          }
          title={`${sourcesReady} of ${sourcesTotal} sources ready`}
          subtitle={
            missingSources.length
              ? `Missing: ${missingSources.join(", ")}`
              : "Catalogs, fingerprinting and tools are all available"
          }
        />
        <Cell
          lead={<Icon name="folder" />}
          title="Folders"
          subtitle={`${setup.library_dir || "library"} · inbox checked every ${setup.inbox_check_seconds} s · trash kept ${setup.trash_days} days`}
        />
        <Cell
          lead={<Icon name="music" />}
          title="Downloads and extras"
          subtitle={[
            setup.ytdlp
              ? `${setup.audio_format === "best" ? "source codec kept" : setup.audio_format.toUpperCase()}, ${setup.max_parallel_downloads} at a time`
              : "yt-dlp missing: links cannot be downloaded",
            setup.lyrics ? "lyrics on" : "lyrics off",
            setup.rsgain ? "album loudness" : "track loudness only",
          ].join(" · ")}
        />
        {!telegram && me.data?.via === "session" && (
          <Cell
            tone="link"
            lead={<Icon name="x" />}
            title="Log out of this browser"
            onClick={() => void logOut()}
          />
        )}
      </Section>
    </>
  );

  const panel = open && (
    <ArtistActions
      key={open.id}
      artist={open}
      others={all}
      feedback={feedback}
      onDone={() => setOpenId(undefined)}
    />
  );

  if (wide)
    return (
      <>
        <section className="pane">
          <div className="screen">{content}</div>
        </section>
        <aside className="detail">
          <div className="screen">
            {open ? (
              <>
                <PageTitle
                  title={open.name}
                  subtitle={`${open.songs} ${open.songs === 1 ? "song" : "songs"}`}
                />
                {panel}
              </>
            ) : (
              <EmptyState
                icon="user"
                title="Pick an artist"
                description="Rename it, join it with another, or split a duet."
              />
            )}
          </div>
        </aside>
      </>
    );

  return (
    <main className="screen">
      {content}
      {open && (
        <Sheet
          title={open.name}
          subtitle={`${open.songs} ${open.songs === 1 ? "song" : "songs"}${open.spellings.length ? ` · also ${open.spellings.join(", ")}` : ""}`}
          onClose={() => setOpenId(undefined)}
        >
          {panel}
        </Sheet>
      )}
    </main>
  );
}

function ArtistActions({
  artist,
  others,
  feedback,
  onDone,
}: {
  artist: ArtistSummary;
  others: ArtistSummary[];
  feedback: Feedback;
  onDone: () => void;
}) {
  const queryClient = useQueryClient();
  const [mode, setMode] = useState<"menu" | "rename" | "join" | "split">("menu");
  const [busy, setBusy] = useState(false);
  const [name, setName] = useState(artist.name);
  const [into, setInto] = useState("");
  const [parts, setParts] = useState(
    artist.name
      .split(SEPARATORS)
      .filter((part) => part.trim())
      .join(", "),
  );
  const songs = `${artist.songs} ${artist.songs === 1 ? "song" : "songs"}`;
  const splitParts = parts
    .split(",")
    .map((part) => part.trim())
    .filter(Boolean);
  const target = others.find((other) => other.name === into && other.id !== artist.id);

  const change = async (body: object, question?: string) => {
    if (question && !(await askConfirm(question))) return;
    setBusy(true);
    try {
      const result = await post<ArtistChanged>(`/artists/${artist.id}`, body);
      haptic.success();
      feedback.notify(
        result.retagged || result.moved
          ? `Done: ${result.retagged} songs retagged, ${result.moved} moved`
          : "Done",
      );
      await queryClient.invalidateQueries({ queryKey: queryKeys.artists });
      onDone();
    } catch (reason) {
      haptic.error();
      feedback.fail((reason as Error).message);
    } finally {
      setBusy(false);
    }
  };

  if (mode === "rename")
    return (
      <Section title="New name" footer={`${songs}: ${MOVE_WARNING}`}>
        <div style={{ padding: 6 }}>
          <Field
            id="artist-name"
            value={name}
            dir="auto"
            onChange={(event) => setName(event.target.value)}
            action={
              <button
                type="button"
                className="small-button filled"
                disabled={busy || !name.trim() || name.trim() === artist.name}
                onClick={() => void change({ action: "rename", name: name.trim() })}
              >
                {busy ? <Spinner size={16} /> : "Rename"}
              </button>
            }
          />
        </div>
        <Cell tone="link" title="Back" onClick={() => setMode("menu")} />
      </Section>
    );

  if (mode === "join")
    return (
      <Section title="Same artist as" footer={`${songs} join that artist. ${MOVE_WARNING}`}>
        <div style={{ padding: 6 }}>
          <Field
            id="artist-into"
            list="artist-names"
            value={into}
            dir="auto"
            placeholder="Another artist…"
            onChange={(event) => setInto(event.target.value)}
            action={
              <button
                type="button"
                className="small-button filled"
                disabled={busy || !target}
                onClick={() => target && void change({ action: "merge", into: target.id })}
              >
                {busy ? <Spinner size={16} /> : "Join"}
              </button>
            }
          />
          <datalist id="artist-names">
            {others
              .filter((other) => other.id !== artist.id)
              .map((other) => (
                <option key={other.id} value={other.name} />
              ))}
          </datalist>
        </div>
        <Cell tone="link" title="Back" onClick={() => setMode("menu")} />
      </Section>
    );

  if (mode === "split")
    return (
      <Section
        title="Artists, separated by commas"
        footer={`Songs go to the folder of the first one; every artist gets credit. ${songs}: ${MOVE_WARNING}`}
      >
        <div style={{ padding: 6 }}>
          <Field
            id="artist-parts"
            value={parts}
            dir="auto"
            onChange={(event) => setParts(event.target.value)}
            action={
              <button
                type="button"
                className="small-button filled"
                disabled={busy || splitParts.length < 2}
                onClick={() => void change({ action: "split", parts: splitParts })}
              >
                {busy ? <Spinner size={16} /> : "Split"}
              </button>
            }
          />
        </div>
        <Cell tone="link" title="Back" onClick={() => setMode("menu")} />
      </Section>
    );

  return (
    <Section
      footer={artist.looks_like_several ? "This name looks like two artists in one." : undefined}
    >
      {artist.looks_like_several && (
        <>
          <Cell
            tone="accent"
            lead={<Icon name="sparkles" />}
            title={`Split into ${splitParts.join(" and ")}`}
            subtitle={`Songs move to ${splitParts[0]}’s folder; both get credit`}
            disabled={busy || splitParts.length < 2}
            onClick={() => void change({ action: "split", parts: splitParts })}
          />
          <Cell
            tone="link"
            lead={<Icon name="check" />}
            title="It’s one artist (a band or duo)"
            disabled={busy}
            onClick={() => void change({ action: "one_artist" })}
          />
        </>
      )}
      <Cell
        tone="link"
        lead={<Icon name="edit" />}
        title="Rename…"
        chevron
        onClick={() => setMode("rename")}
      />
      <Cell
        tone="link"
        lead={<Icon name="user" />}
        title="Same artist as…"
        chevron
        onClick={() => setMode("join")}
      />
      {!artist.looks_like_several && (
        <Cell
          tone="link"
          lead={<Icon name="sparkles" />}
          title="Split into several artists…"
          chevron
          onClick={() => setMode("split")}
        />
      )}
      {busy && <Cell title="Working…" lead={<Spinner />} />}
    </Section>
  );
}

function addedWhen(iso: string) {
  const date = new Date(iso);
  const today = new Date();
  if (date.toDateString() === today.toDateString())
    return date.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
  const yesterday = new Date(today);
  yesterday.setDate(today.getDate() - 1);
  if (date.toDateString() === yesterday.toDateString()) return "Yesterday";
  return date.toLocaleDateString(undefined, { day: "numeric", month: "short" });
}
