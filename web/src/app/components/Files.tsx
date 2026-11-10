import { Fragment, useEffect, useState } from "react";
import { type QueryClient, useQueryClient } from "@tanstack/react-query";
import { api, ApiError } from "../../api/client";
import {
  queryKeys,
  useFileSearchQuery,
  useFolderQuery,
  useFolderTagsQuery,
  useSongDetailsQuery,
} from "../../api/queries";
import type {
  FolderTags,
  LibraryFolderEntry,
  LibrarySong,
  SongChanges,
  SongDetails,
  SongSaved,
  SongsSaved,
} from "../../api/types";
import { Icon } from "../Icons";
import { haptic } from "../telegram";
import { formatDuration } from "../trackUtils";
import { LibraryCover } from "./Cover";
import { EmptyState, ListSkeleton } from "./EmptyStates";
import { Player } from "./SongScreen";
import type { Feedback } from "./ReviewList";
import { BottomActions, Callout, Cell, PageTitle, Section, Sheet, Spinner, Tag } from "./ui";

/** What the app shell needs from the Files tab on a phone. */
export type FilesNav = { back?: () => void; song: boolean };

const parentOf = (path: string) => path.split("/").slice(0, -1).join("/");
const nameOf = (path: string) => path.split("/").pop() || "library";
const count = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** The library folder as it is on disk: browse it, search it, fix one song's tags. */
export function Files({
  wide,
  feedback,
  onNav,
}: {
  wide: boolean;
  feedback: Feedback;
  onNav: (nav: FilesNav) => void;
}) {
  const [folder, setFolder] = useState("");
  const [songPath, setSongPath] = useState<string>();
  const [search, setSearch] = useState("");
  const [onlyHere, setOnlyHere] = useState(false);
  const [open, setOpen] = useState<Set<string>>(() => new Set());
  const [editFolder, setEditFolder] = useState<string>();
  const folderEditor = editFolder !== undefined && (
    <FolderEditor
      key={editFolder}
      path={editFolder}
      feedback={feedback}
      onClose={() => setEditFolder(undefined)}
    />
  );
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
                  onEditFolder={setEditFolder}
                />
              </div>
            )}
          </div>
        </section>
        <aside className="detail">
          <div className="screen">
            {songPath ? (
              <SongView key={songPath} path={songPath} feedback={feedback} />
            ) : (
              <EmptyState
                icon="folder"
                title="Pick a song"
                description="Its tags, cover and file details open here."
              />
            )}
          </div>
        </aside>
        {folderEditor}
      </>
    );

  if (songPath)
    return (
      <main className="screen no-tabs">
        <SongView
          key={songPath}
          path={songPath}
          feedback={feedback}
          onBack={() => setSongPath(undefined)}
        />
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
        <FolderView
          folder={folder}
          searchBox={searchBox}
          onFolder={openFolder}
          onSong={openSong}
          onEditAll={() => setEditFolder(folder)}
        />
      )}
      {folderEditor}
    </main>
  );
}

/** Phone: one folder at a time, with a breadcrumb back up. */
function FolderView({
  folder,
  searchBox,
  onFolder,
  onSong,
  onEditAll,
}: {
  folder: string;
  searchBox: React.ReactNode;
  onFolder: (path: string) => void;
  onSong: (path: string) => void;
  onEditAll: () => void;
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
            {folder && data.songs.length > 1 && (
              <Section>
                <Cell
                  tone="link"
                  lead={<Icon name="edit" />}
                  title={`Edit all ${data.songs.length} songs…`}
                  subtitle="Album, album artist, artist, year, genre"
                  chevron
                  onClick={() => {
                    haptic.tap();
                    onEditAll();
                  }}
                />
              </Section>
            )}
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
  onEditFolder,
}: {
  path: string;
  depth: number;
  open: Set<string>;
  selected?: string;
  onToggle: (path: string) => void;
  onSong: (path: string) => void;
  onEditFolder: (path: string) => void;
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
            <div className="tree-line">
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
              {entry.folders === 0 && entry.songs > 1 && (
                <button
                  type="button"
                  className="tree-edit"
                  aria-label={`Edit all songs in ${entry.name}`}
                  title="Edit all songs in this folder"
                  onClick={() => onEditFolder(entry.path)}
                >
                  <Icon name="edit" size={15} />
                </button>
              )}
            </div>
            {expanded && (
              <TreeFolder
                path={entry.path}
                depth={depth + 1}
                open={open}
                selected={selected}
                onToggle={onToggle}
                onSong={onSong}
                onEditFolder={onEditFolder}
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
function SongView({
  path,
  onBack,
  feedback,
}: {
  path: string;
  onBack?: () => void;
  feedback: Feedback;
}) {
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
        // A new version (after a save or an undo) starts the form again from the file.
        song && <SongInfo key={song.version} song={song} feedback={feedback} />
      )}
    </>
  );
}

type FieldName = keyof SongChanges;
const fieldLabels: Record<FieldName, string> = {
  title: "Title",
  artist: "Artist",
  album: "Album",
  album_artist: "Album artist",
  track: "Track",
  track_total: "of",
  disc: "Disc",
  disc_total: "of",
  year: "Year",
  genre: "Genre",
  composer: "Composer",
  label: "Label",
  isrc: "ISRC",
};

/** The file's tags as the form shows them. */
function formOf(song: SongDetails): Record<FieldName, string> {
  const text = (value?: string | number | null) => (value == null ? "" : String(value));
  return {
    title: text(song.title),
    artist: text(song.artist),
    album: text(song.album),
    album_artist: text(song.album_artist),
    track: text(song.track),
    track_total: text(song.track_total),
    disc: text(song.disc),
    disc_total: text(song.disc_total),
    year: text(song.year),
    genre: text(song.genre),
    composer: text(song.composer),
    label: text(song.label),
    isrc: text(song.isrc),
  };
}

function SongInfo({ song, feedback }: { song: SongDetails; feedback: Feedback }) {
  const queryClient = useQueryClient();
  const original = formOf(song);
  const [form, setForm] = useState(original);
  const [busy, setBusy] = useState(false);
  const changed = (Object.keys(form) as FieldName[]).filter(
    (name) => form[name].trim() !== original[name],
  );
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

  /** Saves `changes` over `version`; the old values come back for Undo. */
  const write = async (version: string, changes: SongChanges) => {
    const saved = await api<SongSaved>("/files/song", {
      method: "PUT",
      body: JSON.stringify({ path: song.path, version, changes }),
    });
    queryClient.setQueryData([...queryKeys.files, "song", song.path], saved.song);
    await refreshAfterSave(queryClient);
    return saved;
  };

  const save = async () => {
    setBusy(true);
    try {
      const changes = Object.fromEntries(changed.map((name) => [name, form[name].trim()]));
      const saved = await write(song.version, changes);
      haptic.success();
      if (!saved.changed) {
        feedback.notify("Nothing to save: the file already says that");
        return;
      }
      feedback.notify(`Saved ${count(saved.changed, "change", "changes")}`, async () => {
        await write(saved.song.version, saved.before);
        haptic.success();
        feedback.notify("Changes undone");
      });
    } catch (reason) {
      haptic.error();
      feedback.fail((reason as Error).message);
      // Someone or something else changed the file: show what it says now.
      if (reason instanceof ApiError && reason.status === 409)
        void queryClient.invalidateQueries({ queryKey: [...queryKeys.files, "song", song.path] });
    } finally {
      setBusy(false);
    }
  };

  const field = (name: FieldName, numeric = false) => (
    <input
      id={`file-${name}`}
      aria-label={fieldLabels[name] === "of" ? `${name.replace("_", " ")}` : fieldLabels[name]}
      dir="auto"
      value={form[name]}
      disabled={!song.editable || busy}
      inputMode={numeric ? "numeric" : undefined}
      placeholder={numeric ? "–" : "Empty"}
      autoComplete="off"
      onChange={(event) => setForm({ ...form, [name]: event.target.value })}
    />
  );
  const row = (name: FieldName, numeric = false) => {
    const isChanged = changed.includes(name);
    return (
      <label className={`field-row ${isChanged ? "changed" : ""}`} key={name}>
        <span>{fieldLabels[name]}</span>
        {field(name, numeric)}
        {isChanged && (
          <small className="was">was {original[name] ? `“${original[name]}”` : "empty"}</small>
        )}
      </label>
    );
  };
  const pair = (name: FieldName, total: FieldName) => {
    const isChanged = changed.includes(name) || changed.includes(total);
    return (
      <label className={`field-row ${isChanged ? "changed" : ""}`} key={name}>
        <span>{fieldLabels[name]}</span>
        <span className="number-pair">
          {field(name, true)}
          <i>of</i>
          {field(total, true)}
        </span>
        {isChanged && (
          <small className="was">
            was {original[name] || "–"} of {original[total] || "–"}
          </small>
        )}
      </label>
    );
  };

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

      <Section
        title="Tags"
        aside={
          changed.length > 0 && (
            <span className="changed-note">{count(changed.length, "change", "changes")}</span>
          )
        }
        footer={
          song.editable
            ? "Only the fields you change are written. Duets and features (“A feat. B”) get credit for each artist in Navidrome."
            : "Tags of WAV and AIFF files are not edited here."
        }
      >
        {row("title")}
        {row("artist")}
        {row("album")}
        {row("album_artist")}
        {pair("track", "track_total")}
        {pair("disc", "disc_total")}
        {row("year", true)}
        {row("genre")}
      </Section>

      <Section title="More tags">
        {row("composer")}
        {row("label")}
        {row("isrc")}
        {song.release_date && <Info label="Release date" value={song.release_date} />}
        {lyrics && (
          <Info
            label="Lyrics"
            value={[synced ? "Synced" : "Plain", firstLine && `“${firstLine}…”`]
              .filter(Boolean)
              .join(" · ")}
          />
        )}
      </Section>

      <Section
        title="File"
        footer="MusicBrainz IDs and loudness are written by magpie-fi when it files a song. A new artist drops that artist’s old MusicBrainz ID."
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

      <BottomActions
        primary={
          changed.length
            ? {
                text: `Save ${count(changed.length, "change", "changes")}`,
                onClick: () => void save(),
                busy,
              }
            : undefined
        }
        secondary={
          changed.length
            ? { text: "Undo changes", onClick: () => setForm(original), disabled: busy }
            : undefined
        }
      />
    </>
  );
}

/** Lists, search, open songs and the Artists page show the new tags. */
function refreshAfterSave(queryClient: QueryClient) {
  return Promise.all([
    queryClient.invalidateQueries({ queryKey: queryKeys.files }),
    queryClient.invalidateQueries({ queryKey: queryKeys.artists }),
  ]);
}

type FolderField = keyof FolderTags["fields"];
const folderFields: Array<[FolderField, string]> = [
  ["album", "Album"],
  ["album_artist", "Album artist"],
  ["artist", "Artist"],
  ["year", "Year"],
  ["genre", "Genre"],
];

/** "Edit all songs": the album-level fields of every song directly in a folder. */
function FolderEditor({
  path,
  feedback,
  onClose,
}: {
  path: string;
  feedback: Feedback;
  onClose: () => void;
}) {
  const queryClient = useQueryClient();
  const summary = useFolderTagsQuery(path);
  const [typed, setTyped] = useState<Partial<Record<FolderField, string>>>({});
  const [busy, setBusy] = useState(false);
  const data = summary.data;
  const songs = data?.songs.filter((song) => song.editable) ?? [];
  const skipped = (data?.songs.length ?? 0) - songs.length;
  const shared = (field: FolderField) => data?.fields[field] ?? { value: null, values: 0 };
  const initial = (field: FolderField) => shared(field).value ?? "";
  const changed = folderFields
    .map(([field]) => field)
    .filter((field) => typed[field] !== undefined && typed[field]!.trim() !== initial(field));

  const put = (edits: Array<{ path: string; version: string; changes: SongChanges }>) =>
    api<SongsSaved>("/files/songs", { method: "PUT", body: JSON.stringify({ edits }) });

  const save = async () => {
    setBusy(true);
    try {
      const changes = Object.fromEntries(changed.map((field) => [field, typed[field]!.trim()]));
      const result = await put(
        songs.map((song) => ({ path: song.path, version: song.version, changes })),
      );
      await refreshAfterSave(queryClient);
      const done = result.saved.filter((song) => song.changed > 0);
      if (result.failed.length) {
        haptic.error();
        feedback.fail(
          `${count(result.failed.length, "song", "songs")} could not be saved: ${result.failed[0].error}`,
        );
      } else haptic.success();
      if (done.length)
        feedback.notify(`Saved to ${count(done.length, "song", "songs")}`, async () => {
          await put(
            done.map((song) => ({
              path: song.song.path,
              version: song.song.version,
              changes: song.before,
            })),
          );
          await refreshAfterSave(queryClient);
          haptic.success();
          feedback.notify("Changes undone");
        });
      else if (!result.failed.length)
        feedback.notify("Nothing to save: the songs already say that");
      onClose();
    } catch (reason) {
      haptic.error();
      feedback.fail((reason as Error).message);
      if (reason instanceof ApiError && reason.status === 409) void summary.refetch();
    } finally {
      setBusy(false);
    }
  };

  return (
    <Sheet
      title={`Edit all ${count(songs.length, "song", "songs")}`}
      subtitle={nameOf(path)}
      onClose={onClose}
    >
      {summary.isPending ? (
        <ListSkeleton rows={3} />
      ) : summary.error ? (
        <Callout tone="bad" icon="alert">
          {summary.error.message}
        </Callout>
      ) : (
        <>
          <Section
            footer={[
              "Only the fields you change are written, to every song directly in this folder. “Different” means the songs disagree; leave it to keep each song’s own value.",
              skipped > 0 &&
                `${count(skipped, "WAV or AIFF song is", "WAV or AIFF songs are")} left as they are.`,
            ]
              .filter(Boolean)
              .join(" ")}
          >
            {folderFields.map(([field, label]) => {
              const isChanged = changed.includes(field);
              const { value, values } = shared(field);
              return (
                <label className={`field-row ${isChanged ? "changed" : ""}`} key={field}>
                  <span>{label}</span>
                  <input
                    id={`folder-${field}`}
                    dir="auto"
                    value={typed[field] ?? initial(field)}
                    placeholder={values > 1 ? `Different (${values} values)` : "Empty"}
                    inputMode={field === "year" ? "numeric" : undefined}
                    autoComplete="off"
                    disabled={busy}
                    onChange={(event) => setTyped({ ...typed, [field]: event.target.value })}
                  />
                  {isChanged && (
                    <small className="was">
                      {values > 1
                        ? `replaces ${values} different values`
                        : value
                          ? `was “${value}”`
                          : "was empty"}
                    </small>
                  )}
                </label>
              );
            })}
          </Section>
          <div className="sheet-actions">
            <button type="button" className="action-button secondary" onClick={onClose}>
              Cancel
            </button>
            <button
              type="button"
              className="action-button primary"
              disabled={!changed.length || !songs.length || busy}
              onClick={() => void save()}
            >
              {busy ? <Spinner size={18} /> : `Save to ${count(songs.length, "song", "songs")}`}
            </button>
          </div>
        </>
      )}
    </Sheet>
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
