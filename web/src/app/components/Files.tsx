import { Fragment, useEffect, useState } from "react";
import { useFileSearchQuery, useFolderQuery, useSongDetailsQuery } from "../../api/queries";
import type { LibraryFolderEntry, LibrarySong, SongDetails } from "../../api/types";
import { Icon } from "../Icons";
import { haptic } from "../telegram";
import { formatDuration } from "../trackUtils";
import { LibraryCover } from "./Cover";
import { EmptyState, ListSkeleton } from "./EmptyStates";
import { Player } from "./SongScreen";
import { Callout, Cell, PageTitle, Section, Tag } from "./ui";

/** What the app shell needs from the Files tab on a phone. */
export type FilesNav = { back?: () => void; song: boolean };

const parentOf = (path: string) => path.split("/").slice(0, -1).join("/");
const nameOf = (path: string) => path.split("/").pop() || "library";
const count = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** The library folder as it is on disk: browse it, search it, look at one song. */
export function Files({ wide, onNav }: { wide: boolean; onNav: (nav: FilesNav) => void }) {
  const [folder, setFolder] = useState("");
  const [songPath, setSongPath] = useState<string>();
  const [search, setSearch] = useState("");
  const [onlyHere, setOnlyHere] = useState(false);
  const [open, setOpen] = useState<Set<string>>(() => new Set());
  const query = useDebounced(search.trim(), 250);
  const within = !wide && onlyHere ? folder : "";

  const openFolder = (path: string) => {
    haptic.tap();
    setFolder(path);
    setSongPath(undefined);
    setSearch("");
    setOnlyHere(false);
    // On a wide screen the tree opens down to that folder.
    setOpen((current) => {
      const next = new Set(current);
      const parts = path.split("/");
      parts.forEach((_, index) => next.add(parts.slice(0, index + 1).join("/")));
      return next;
    });
    if (!wide) window.scrollTo({ top: 0 });
  };
  const openSong = (path: string) => {
    haptic.tap();
    setSongPath(path);
    if (!wide) window.scrollTo({ top: 0 });
  };

  // Telegram's back button: a song closes first, then each folder goes up one level.
  const searching = search.trim().length > 0;
  useEffect(() => {
    const back = wide
      ? undefined
      : songPath
        ? () => setSongPath(undefined)
        : folder && !searching
          ? () => {
              setFolder(parentOf(folder));
              window.scrollTo({ top: 0 });
            }
          : undefined;
    onNav({ back, song: !wide && Boolean(songPath) });
  }, [wide, songPath, folder, searching, onNav]);
  useEffect(() => () => onNav({ song: false }), [onNav]);

  const searchBox = (
    <>
      <label className="search-field">
        <Icon name="search" size={18} />
        <span className="sr-only">Search the library</span>
        <input
          id="file-search"
          value={search}
          onChange={(event) => setSearch(event.target.value)}
          placeholder="Search songs, artists, albums"
          autoComplete="off"
          enterKeyHint="search"
          dir="auto"
        />
        {search && (
          <button
            type="button"
            className="search-clear"
            aria-label="Clear search"
            onClick={() => setSearch("")}
          >
            <Icon name="x" size={12} />
          </button>
        )}
      </label>
      {search && !wide && folder && (
        <div className="search-scope" role="group" aria-label="Search in">
          <button
            type="button"
            className={onlyHere ? "" : "on"}
            aria-pressed={!onlyHere}
            onClick={() => setOnlyHere(false)}
          >
            Everywhere
          </button>
          <button
            type="button"
            className={onlyHere ? "on" : ""}
            aria-pressed={onlyHere}
            onClick={() => setOnlyHere(true)}
            dir="auto"
          >
            In {nameOf(folder)}
          </button>
        </div>
      )}
    </>
  );

  const results = searching ? (
    <SearchResults
      query={query}
      within={within}
      selected={songPath}
      onFolder={openFolder}
      onSong={openSong}
    />
  ) : null;

  if (wide)
    return (
      <>
        <section className="pane">
          <div className="screen">
            <PageTitle title="Files" subtitle="Your library folder, as Navidrome sees it." />
            {searchBox}
            {results ?? (
              <div className="tree" role="tree" aria-label="Library folders">
                <TreeFolder
                  path=""
                  depth={0}
                  open={open}
                  selected={songPath}
                  onToggle={(path) => {
                    setOpen((current) => {
                      const next = new Set(current);
                      if (next.has(path)) next.delete(path);
                      else next.add(path);
                      return next;
                    });
                  }}
                  onSong={openSong}
                />
              </div>
            )}
          </div>
        </section>
        <aside className="detail">
          <div className="screen">
            {songPath ? (
              <SongView key={songPath} path={songPath} />
            ) : (
              <EmptyState
                icon="folder"
                title="Pick a song"
                description="Its tags, cover and file details open here."
              />
            )}
          </div>
        </aside>
      </>
    );

  if (songPath)
    return (
      <main className="screen no-tabs">
        <SongView key={songPath} path={songPath} onBack={() => setSongPath(undefined)} />
      </main>
    );

  return (
    <main className="screen">
      {results ? (
        <>
          <PageTitle title="Files" subtitle="Your library folder, as Navidrome sees it." />
          {searchBox}
          {results}
        </>
      ) : (
        <FolderView folder={folder} searchBox={searchBox} onFolder={openFolder} onSong={openSong} />
      )}
    </main>
  );
}

/** Phone: one folder at a time, with a breadcrumb back up. */
function FolderView({
  folder,
  searchBox,
  onFolder,
  onSong,
}: {
  folder: string;
  searchBox: React.ReactNode;
  onFolder: (path: string) => void;
  onSong: (path: string) => void;
}) {
  const listing = useFolderQuery(folder);
  const data = listing.data;
  const parts = folder ? folder.split("/") : [];
  return (
    <>
      <PageTitle
        title={folder ? nameOf(folder) : "Files"}
        subtitle={
          folder
            ? data && count(data.songs_inside, "song", "songs")
            : "Your library folder, as Navidrome sees it."
        }
        onBack={folder ? () => onFolder(parentOf(folder)) : undefined}
      />
      {searchBox}
      <nav className="crumbs" aria-label="Folder">
        {["", ...parts.map((_, index) => parts.slice(0, index + 1).join("/"))].map(
          (path, index) => (
            <span key={path || "library"} className={index === parts.length ? "here" : ""}>
              {index > 0 && <Icon name="chevron" size={12} />}
              <button
                type="button"
                dir="auto"
                disabled={index === parts.length}
                onClick={() => onFolder(path)}
              >
                {path ? nameOf(path) : "library"}
              </button>
            </span>
          ),
        )}
      </nav>
      {listing.isPending ? (
        <section className="section">
          <ListSkeleton rows={5} />
        </section>
      ) : listing.error ? (
        <div className="section">
          <Callout tone="bad" icon="alert">
            {listing.error.message}
          </Callout>
        </div>
      ) : data && data.folders.length + data.songs.length === 0 ? (
        <EmptyState
          icon="folder"
          title="Nothing here"
          description="This folder has no songs or folders."
        />
      ) : (
        data && (
          <>
            {data.folders.length > 0 && (
              <Section
                title="Folders"
                aside={!folder ? count(data.songs_inside, "song", "songs") : undefined}
              >
                {data.folders.map((entry) => (
                  <FolderRow key={entry.path} entry={entry} onOpen={onFolder} />
                ))}
              </Section>
            )}
            {data.songs.length > 0 && (
              <Section
                title={folder ? "Songs" : "Loose files"}
                footer={
                  folder ? undefined : "Songs directly in library/. Navidrome still plays them."
                }
              >
                {data.songs.map((song) => (
                  <SongRow key={song.path} song={song} onOpen={onSong} />
                ))}
              </Section>
            )}
          </>
        )
      )}
    </>
  );
}

function FolderRow({
  entry,
  subtitle,
  onOpen,
}: {
  entry: LibraryFolderEntry;
  subtitle?: string;
  onOpen: (path: string) => void;
}) {
  return (
    <Cell
      lead={
        <span className="folder-lead">
          <Icon name="folder" size={20} />
        </span>
      }
      title={entry.name}
      subtitle={[
        subtitle,
        entry.folders > 0 && count(entry.folders, "folder", "folders"),
        count(entry.songs, "song", "songs"),
      ]
        .filter(Boolean)
        .join(" · ")}
      chevron
      onClick={() => onOpen(entry.path)}
    />
  );
}

function SongRow({
  song,
  showFolder,
  selected,
  onOpen,
}: {
  song: LibrarySong;
  showFolder?: boolean;
  selected?: boolean;
  onOpen: (path: string) => void;
}) {
  const folder = parentOf(song.path).split("/").join(" / ");
  return (
    <Cell
      lead={<LibraryCover path={song.path} size="small" />}
      title={song.title}
      subtitle={
        [song.artist, showFolder ? folder || "library" : undefined].filter(Boolean).join(" · ") ||
        song.name
      }
      trailing={
        <span className="file-meta">
          <Tag tone="gray">{song.format.toUpperCase()}</Tag>
          {formatDuration(song.duration ?? undefined)}
        </span>
      }
      selected={selected}
      onClick={() => onOpen(song.path)}
    />
  );
}

function SearchResults({
  query,
  within,
  selected,
  onFolder,
  onSong,
}: {
  query: string;
  within: string;
  selected?: string;
  onFolder: (path: string) => void;
  onSong: (path: string) => void;
}) {
  const search = useFileSearchQuery(query, within);
  const data = search.data;
  // Waiting for the typing pause, or for the first answer.
  if (!query || search.isPending)
    return (
      <section className="section">
        <ListSkeleton rows={4} />
      </section>
    );
  if (search.error)
    return (
      <div className="section">
        <Callout tone="bad" icon="alert">
          {search.error.message}
        </Callout>
      </div>
    );
  if (!data || data.folders.length + data.songs.length === 0)
    return (
      <EmptyState
        icon="search"
        title="Nothing found"
        description="Try fewer words, or another spelling."
      />
    );
  return (
    <>
      {data.folders.length > 0 && (
        <Section title="Folders" aside={data.folders.length}>
          {data.folders.map((entry) => (
            <FolderRow
              key={entry.path}
              entry={entry}
              subtitle={parentOf(entry.path).split("/").join(" / ") || undefined}
              onOpen={onFolder}
            />
          ))}
        </Section>
      )}
      {data.songs.length > 0 && (
        <Section
          title="Songs"
          aside={data.more ? `first ${data.songs.length}` : data.songs.length}
          footer={data.more ? "More songs match. Add a word to narrow it down." : undefined}
        >
          {data.songs.map((song) => (
            <SongRow
              key={song.path}
              song={song}
              showFolder
              selected={song.path === selected}
              onOpen={onSong}
            />
          ))}
        </Section>
      )}
    </>
  );
}

/** Wide screens: folders open in place. A folder loads only when it is opened. */
function TreeFolder({
  path,
  depth,
  open,
  selected,
  onToggle,
  onSong,
}: {
  path: string;
  depth: number;
  open: Set<string>;
  selected?: string;
  onToggle: (path: string) => void;
  onSong: (path: string) => void;
}) {
  const listing = useFolderQuery(path);
  const indent = { paddingInlineStart: 12 + depth * 18 };
  if (listing.isPending)
    return (
      <div className="tree-row tree-note" style={indent}>
        Loading…
      </div>
    );
  if (listing.error)
    return (
      <div className="tree-row tree-note" style={indent}>
        {listing.error.message}
      </div>
    );
  const data = listing.data;
  if (data.folders.length + data.songs.length === 0)
    return (
      <div className="tree-row tree-note" style={indent}>
        Empty folder
      </div>
    );
  return (
    <>
      {data.folders.map((entry) => {
        const expanded = open.has(entry.path);
        return (
          <div key={entry.path} role="group">
            <button
              type="button"
              className="tree-row"
              style={indent}
              role="treeitem"
              aria-expanded={expanded}
              onClick={() => onToggle(entry.path)}
            >
              <Icon name="chevron" size={14} className={`twisty ${expanded ? "open" : ""}`} />
              <Icon name="folder" size={18} className="tree-folder" />
              <span className="tree-name" dir="auto">
                {entry.name}
              </span>
              <small>{entry.songs}</small>
            </button>
            {expanded && (
              <TreeFolder
                path={entry.path}
                depth={depth + 1}
                open={open}
                selected={selected}
                onToggle={onToggle}
                onSong={onSong}
              />
            )}
          </div>
        );
      })}
      {data.songs.map((song) => (
        <button
          type="button"
          key={song.path}
          className={`tree-row ${song.path === selected ? "selected" : ""}`}
          style={indent}
          role="treeitem"
          aria-selected={song.path === selected}
          onClick={() => onSong(song.path)}
        >
          <span className="twisty" />
          <Icon name="music" size={16} className="tree-folder" />
          <span className="tree-name" dir="auto">
            {song.name.replace(/\.[^/.]+$/, "")}
          </span>
          <small>{formatDuration(song.duration ?? undefined)}</small>
        </button>
      ))}
    </>
  );
}

/** Everything one song's file says about itself. */
function SongView({ path, onBack }: { path: string; onBack?: () => void }) {
  const details = useSongDetailsQuery(path);
  const song = details.data;
  return (
    <>
      <PageTitle title="Song" subtitle={nameOf(path)} onBack={onBack} />
      {details.isPending ? (
        <section className="section">
          <ListSkeleton rows={3} />
        </section>
      ) : details.error ? (
        <div className="section">
          <Callout tone="bad" icon="alert">
            {details.error.message}
          </Callout>
        </div>
      ) : (
        song && <SongInfo song={song} />
      )}
    </>
  );
}

function SongInfo({ song }: { song: SongDetails }) {
  const of = (number?: number | null, total?: number | null) =>
    number ? (total ? `${number} / ${total}` : String(number)) : undefined;
  const sound = [
    song.format.toUpperCase(),
    song.bitrate && `${song.bitrate} kbps`,
    song.sample_rate && `${(song.sample_rate / 1000).toLocaleString()} kHz`,
    song.bit_depth && `${song.bit_depth}-bit`,
    formatDuration(song.duration),
    formatSize(song.size),
  ].filter(Boolean);
  const lyrics = song.lyrics?.trim();
  const synced = lyrics ? /^\s*\[\d+:\d+/m.test(lyrics) : false;
  const firstLine = lyrics
    ?.split("\n")
    .map((line) => line.replace(/^\s*(\[[^\]]*\])+/, "").trim())
    .find(Boolean);
  const loudness = [
    song.track_gain && `${song.track_gain} track`,
    song.album_gain && `${song.album_gain} album`,
  ].filter(Boolean);
  const more: Array<[string, string | undefined | null]> = [
    ["Composer", song.composer],
    ["Label", song.label],
    ["ISRC", song.isrc],
    ["Release date", song.release_date],
  ];
  return (
    <>
      <div className="hero">
        <LibraryCover path={song.path} size="large" />
        <div className="hero-main">
          <h2 dir="auto">{song.title || nameOf(song.path)}</h2>
          {song.artist && <p dir="auto">{song.artist}</p>}
          <Player
            key={song.path}
            src={`/api/files/audio?path=${encodeURIComponent(song.path)}`}
            duration={song.duration}
          />
          <p className="hero-facts">{sound.join(" · ")}</p>
        </div>
      </div>

      <Section title="Tags">
        <Info label="Title" value={song.title} />
        <Info label="Artist" value={song.artist} />
        {song.artists.length > 1 && <Info label="Artists" value={song.artists.join(", ")} />}
        <Info label="Album" value={song.album} />
        <Info label="Album artist" value={song.album_artist} />
        <Info label="Track" value={of(song.track, song.track_total)} />
        <Info label="Disc" value={of(song.disc, song.disc_total)} />
        <Info label="Year" value={song.year} />
        <Info label="Genre" value={song.genre} />
      </Section>

      {(more.some(([, value]) => value) || lyrics) && (
        <Section title="More tags">
          {more
            .filter(([, value]) => value)
            .map(([label, value]) => (
              <Info key={label} label={label} value={value} />
            ))}
          {lyrics && (
            <Info
              label="Lyrics"
              value={[synced ? "Synced" : "Plain", firstLine && `“${firstLine}…”`]
                .filter(Boolean)
                .join(" · ")}
            />
          )}
        </Section>
      )}

      <Section
        title="File"
        footer="MusicBrainz IDs and loudness are written by magpie-fi when it files a song."
      >
        <Info
          label="Cover"
          value={song.has_cover ? "Inside the file" : undefined}
          empty="None inside the file"
        />
        <Info
          label="MusicBrainz"
          value={song.musicbrainz_recording ? "Linked" : undefined}
          empty="Not linked"
          tone={song.musicbrainz_recording ? "good" : undefined}
        />
        {loudness.length > 0 && <Info label="Loudness" value={loudness.join(" · ")} />}
        <Info label="Path" value={song.path} mono />
      </Section>
    </>
  );
}

function Info({
  label,
  value,
  empty = "—",
  mono,
  tone,
}: {
  label: string;
  value?: string | null;
  empty?: string;
  mono?: boolean;
  tone?: "good";
}) {
  return (
    <div className="info-row">
      <span>{label}</span>
      {value ? (
        tone ? (
          <span>
            <Tag tone={tone}>{value}</Tag>
          </span>
        ) : mono ? (
          <PathText path={value} />
        ) : (
          <b dir="auto">{value}</b>
        )
      ) : (
        <i>{empty}</i>
      )}
    </div>
  );
}

/** A path always reads left to right, folder by folder; each name keeps its own direction. */
function PathText({ path }: { path: string }) {
  return (
    <span dir="ltr" className="path-text mono">
      {path.split("/").map((part, index) => (
        <Fragment key={index}>
          {index > 0 && "/"}
          <bdi>{part}</bdi>
        </Fragment>
      ))}
    </span>
  );
}

function formatSize(bytes: number) {
  if (bytes >= 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  return `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

function useDebounced<T>(value: T, delay: number) {
  const [current, setCurrent] = useState(value);
  useEffect(() => {
    const timer = window.setTimeout(() => setCurrent(value), delay);
    return () => window.clearTimeout(timer);
  }, [value, delay]);
  return current;
}
