import { useMemo, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { post } from "../../api/client";
import { queryKeys, useArtistsQuery, useSplitAlbumsQuery } from "../../api/queries";
import type { ArtistChanged, ArtistSummary, Setup, SplitAlbum } from "../../api/types";
import { Icon } from "../Icons";
import { askConfirm, haptic } from "../telegram";
import { EmptyState, ListSkeleton } from "./EmptyStates";
import type { Feedback } from "./ReviewList";
import { StatusSection } from "./Status";
import { Cell, Field, PageTitle, Section, Sheet, Spinner, Tag } from "./ui";

const SEPARATORS = /\s+(?:&|x|X|×|و|with|With|vs\.?|feat\.?|ft\.?|featuring)\s+|\s*[,،]\s*/;
const MOVE_WARNING =
  "Their tags are rewritten and they move to the right artist folder. Navidrome may count them as new songs, so play counts can start again.";

/** Artists drawn before "Show more". */
const ARTIST_PAGE = 100;

/** The Artists page: every artist in the library, with fixes for wrong names. On a phone or in
    Telegram the server status sits at the bottom; a wide window shows it in the sidebar. */
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
  const [search, setSearch] = useState("");
  const [openId, setOpenId] = useState<number>();
  // A library can have thousands of artists; drawing them all at once makes the page slow.
  const [rows, setRows] = useState(ARTIST_PAGE);
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
  const plain = shown.filter((artist) => !artist.looks_like_several);
  const open = all.find((artist) => artist.id === openId);
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
      trailing={
        artist.looks_like_several ? (
          <Tag tone="warn">Split?</Tag>
        ) : artist.albums_to_check > 0 ? (
          <Tag tone="warn">Albums?</Tag>
        ) : undefined
      }
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
      <PageTitle
        title="Artists"
        subtitle="Rename, join or split the artists of your filed songs."
      />
      <div className="stats">
        <div>
          <b>{setup.library_songs.toLocaleString()}</b>
          <small>songs</small>
        </div>
        <div>
          <b>{artists.isPending ? "…" : all.length.toLocaleString()}</b>
          <small>artists</small>
        </div>
      </div>
      <label className="search-field">
        <Icon name="search" size={18} />
        <span className="sr-only">Search artists</span>
        <input
          id="artist-search"
          value={search}
          onChange={(event) => {
            setSearch(event.target.value);
            setRows(ARTIST_PAGE);
          }}
          placeholder="Search artists"
          autoComplete="off"
        />
      </label>

      {artists.isPending ? (
        <section className="section">
          <ListSkeleton rows={4} />
        </section>
      ) : all.length === 0 ? (
        <EmptyState
          icon="users"
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
          <Section title={suspicious.length > 0 ? "All artists" : undefined}>
            {plain.slice(0, rows).map(artistRow)}
            {plain.length > rows && (
              <Cell
                tone="accent"
                lead={<Icon name="chevron" />}
                title={`Show ${Math.min(ARTIST_PAGE, plain.length - rows)} more`}
                subtitle={`${rows.toLocaleString()} of ${plain.length.toLocaleString()} shown. Searching looks through all of them.`}
                onClick={() => setRows((current) => current + ARTIST_PAGE)}
              />
            )}
            {shown.length === 0 && (
              <Cell title="No artist matches" subtitle="Try another spelling" />
            )}
          </Section>
        </>
      )}

      {!wide && <StatusSection setup={setup} />}
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
    <>
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
      <AlbumFixes artist={artist} feedback={feedback} />
    </>
  );
}

const plural = (count: number, word: string) => `${count} ${word}${count === 1 ? "" : "s"}`;

/** How the other versions of a split album differ, in one line. */
function versionsLine(album: SplitAlbum) {
  return album.versions
    .filter((version) => !version.kept)
    .map((version) => {
      const songs = plural(version.songs, "song");
      const kind = version.kind ? `, ${version.kind}` : "";
      if (version.differs_in.includes("spelling")) return `${songs} as “${version.album}”${kind}`;
      const other = version.differs_in.filter((what) => what !== version.kind);
      return `${songs} with another ${other.join(" and ") || "folder"}${kind}`;
    })
    .join(" · ");
}

/** Albums Navidrome shows twice because their songs disagree on the album's details. */
function AlbumFixes({ artist, feedback }: { artist: ArtistSummary; feedback: Feedback }) {
  const queryClient = useQueryClient();
  const albums = useSplitAlbumsQuery(artist.id);
  const [busyKey, setBusyKey] = useState<string>();
  if (albums.isPending) return <Cell title="Checking the albums…" lead={<Spinner />} />;
  if (!albums.data?.length) return null;

  const join = async (album: SplitAlbum) => {
    const question = `Make “${album.album}” one album? ${plural(album.songs, "song")} get the same album details and the same folder. ${MOVE_WARNING}`;
    if (!(await askConfirm(question))) return;
    setBusyKey(album.key);
    try {
      const result = await post<ArtistChanged>(`/artists/${artist.id}`, {
        action: "join_album",
        key: album.key,
      });
      haptic.success();
      feedback.notify(
        `“${album.album}” is one album: ${result.retagged} songs retagged, ${result.moved} moved`,
      );
      await queryClient.invalidateQueries({ queryKey: queryKeys.artists });
    } catch (reason) {
      haptic.error();
      feedback.fail((reason as Error).message);
    } finally {
      setBusyKey(undefined);
    }
  };

  return (
    <Section
      title="Albums shown twice"
      footer="Navidrome splits an album when its songs disagree on its name, album artist, release date or edition. Every song takes the details most of the album has and moves into its folder."
    >
      {albums.data.map((album) => (
        <Cell
          key={album.key}
          tone="accent"
          lead={busyKey === album.key ? <Spinner /> : <Icon name="sparkles" />}
          title={`Make “${album.album}” one album`}
          subtitle={versionsLine(album)}
          disabled={busyKey !== undefined}
          onClick={() => void join(album)}
        />
      ))}
    </Section>
  );
}
