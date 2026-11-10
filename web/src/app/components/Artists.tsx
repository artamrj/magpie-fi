import { useMemo, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { post } from "../../api/client";
import { queryKeys, useArtistsQuery } from "../../api/queries";
import type { ArtistChanged, ArtistSummary } from "../../api/types";
import { Icon } from "../Icons";
import { askConfirm } from "../telegram";
import { EmptyState } from "./EmptyStates";

const SEPARATORS = /\s+(?:&|x|X|×|و|with|With|vs\.?|feat\.?|ft\.?|featuring)\s+|\s*[,،]\s*/;

const MOVE_WARNING =
  "Their artist tags are rewritten and they move to the right artist folder. Navidrome may see them as new songs, so their play counts and stars can start again.";

/** Every artist in the library, with fixes for names that came out wrong. */
export function Artists({
  onError,
  onNotice,
}: {
  onError: (message: string) => void;
  onNotice: (message: string) => void;
}) {
  const artists = useArtistsQuery();
  const [search, setSearch] = useState("");
  const [open, setOpen] = useState<number>();
  const list = useMemo(() => {
    const all = artists.data ?? [];
    const query = search.trim().toLowerCase();
    const shown = query
      ? all.filter((artist) =>
          [artist.name, ...artist.spellings].some((name) => name.toLowerCase().includes(query)),
        )
      : all;
    // Names that look like several artists come first: they are the likely fixes.
    return [...shown].sort(
      (left, right) => Number(right.looks_like_several) - Number(left.looks_like_several),
    );
  }, [artists.data, search]);

  return (
    <main className="get-music" id="workspace">
      <div className="get-music-form">
        <label htmlFor="artist-search">
          <b>Artists</b>
          <small>
            Every spelling of an artist shares one name and one folder. Fix a name that came out
            wrong, join two that are the same artist, or split a duet into its singers.
          </small>
        </label>
        <div className="get-music-input">
          <input
            id="artist-search"
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            placeholder="Search artists…"
            autoComplete="off"
          />
        </div>
      </div>
      {artists.isPending ? (
        <span className="spinner" />
      ) : list.length === 0 ? (
        <EmptyState
          icon="music"
          title={search ? "No artist matches" : "No artists yet"}
          description="Artists appear here once their songs are in your library."
        />
      ) : (
        <ul className="request-list">
          {list.map((artist) => (
            <ArtistCard
              key={artist.id}
              artist={artist}
              others={artists.data ?? []}
              open={open === artist.id}
              onToggle={() => setOpen(open === artist.id ? undefined : artist.id)}
              onError={onError}
              onNotice={onNotice}
            />
          ))}
        </ul>
      )}
    </main>
  );
}

function ArtistCard({
  artist,
  others,
  open,
  onToggle,
  onError,
  onNotice,
}: {
  artist: ArtistSummary;
  others: ArtistSummary[];
  open: boolean;
  onToggle: () => void;
  onError: (message: string) => void;
  onNotice: (message: string) => void;
}) {
  const queryClient = useQueryClient();
  const [busy, setBusy] = useState(false);
  const [name, setName] = useState(artist.name);
  const [parts, setParts] = useState(
    artist.name
      .split(SEPARATORS)
      .filter((part) => part.trim())
      .join(", "),
  );
  const [into, setInto] = useState("");
  const songs = `${artist.songs} ${artist.songs === 1 ? "song" : "songs"}`;

  const change = async (body: object, question?: string) => {
    if (question && !(await askConfirm(question))) return;
    setBusy(true);
    try {
      const result = await post<ArtistChanged>(`/artists/${artist.id}`, body);
      onNotice(
        result.retagged || result.moved
          ? `Done: ${result.retagged} songs retagged, ${result.moved} moved.`
          : "Done.",
      );
      await queryClient.invalidateQueries({ queryKey: queryKeys.artists });
    } catch (reason) {
      onError((reason as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const target = others.find((other) => other.name === into && other.id !== artist.id);
  const splitParts = parts
    .split(",")
    .map((part) => part.trim())
    .filter(Boolean);

  return (
    <li className="request-card">
      <button className="request-summary" onClick={onToggle} aria-expanded={open}>
        <span className="request-kind">{artist.looks_like_several ? "Several?" : "Artist"}</span>
        <span className="request-title">
          <b>{artist.name}</b>
          <small>
            {songs}
            {artist.spellings.length > 0 && ` · also written ${artist.spellings.join(", ")}`}
          </small>
        </span>
        <span />
        <Icon name="chevron" size={16} />
      </button>
      {open && (
        <div className="request-songs artist-actions">
          <label>
            <span>Name in the tags and the folder</span>
            <span className="get-music-input">
              <input value={name} onChange={(event) => setName(event.target.value)} />
              <button
                className="compact-button"
                disabled={busy || !name.trim() || name.trim() === artist.name}
                onClick={() =>
                  void change(
                    { action: "rename", name: name.trim() },
                    `Rename "${artist.name}" to "${name.trim()}"? ${songs}: ${MOVE_WARNING}`,
                  )
                }
              >
                <Icon name="edit" size={15} />
                Rename
              </button>
            </span>
          </label>
          <label>
            <span>Same artist as</span>
            <span className="get-music-input">
              <input
                list={`artists-${artist.id}`}
                value={into}
                onChange={(event) => setInto(event.target.value)}
                placeholder="Another artist…"
              />
              <datalist id={`artists-${artist.id}`}>
                {others
                  .filter((other) => other.id !== artist.id)
                  .map((other) => (
                    <option key={other.id} value={other.name} />
                  ))}
              </datalist>
              <button
                className="compact-button"
                disabled={busy || !target}
                onClick={() =>
                  target &&
                  void change(
                    { action: "merge", into: target.id },
                    `Join "${artist.name}" into "${target.name}"? ${songs}: ${MOVE_WARNING}`,
                  )
                }
              >
                <Icon name="layers" size={15} />
                Join
              </button>
            </span>
          </label>
          <label>
            <span>Several artists (separate with commas)</span>
            <span className="get-music-input">
              <input value={parts} onChange={(event) => setParts(event.target.value)} />
              <button
                className="compact-button"
                disabled={busy || splitParts.length < 2}
                onClick={() =>
                  void change(
                    { action: "split", parts: splitParts },
                    `Split "${artist.name}" into ${splitParts.join(" and ")}? Its songs go to the folder of ${splitParts[0]}. ${songs}: ${MOVE_WARNING}`,
                  )
                }
              >
                <Icon name="arrow" size={15} />
                Split
              </button>
            </span>
          </label>
          {artist.looks_like_several && (
            <button
              className="compact-button"
              disabled={busy}
              onClick={() => void change({ action: "one_artist" })}
            >
              <Icon name="check" size={15} />
              It is one artist (a band or duo)
            </button>
          )}
          {busy && <span className="spinner" />}
        </div>
      )}
    </li>
  );
}
