import { useState } from "react";
import { api, post } from "../../api/client";
import type { ArtistCredit, Candidate, Track } from "../../api/types";
import { Icon } from "../Icons";
import { Section, Spinner } from "./ui";

const labels: Record<string, string> = {
  title: "Title",
  artist: "Artist",
  album: "Album",
  album_artist: "Album artist",
  track_number: "Track number",
  year: "Year",
  release_date: "Release date",
  genre: "Genre",
  composer: "Composer",
  label: "Label",
  isrc: "ISRC",
  cover_url: "Cover link",
};

/**
 * The chips an artist line stands for: one per artist, split where the tags are split
 * (feat., "/", ";"). "A, B & C" stays one chip, as it is written to the file.
 */
function creditsFromText(text: string): ArtistCredit[] {
  const parts = text.split(/(\s+(?:feat\.?|ft\.?|featuring)\s+|\s+\/\s+|\s*;\s*)/i);
  const credits: ArtistCredit[] = [];
  for (let index = 0; index < parts.length; index += 2) {
    const name = parts[index].trim();
    if (!name) continue;
    const separator = (parts[index + 1] ?? "").trim().toLowerCase();
    const join = !separator ? "" : separator === "/" ? " / " : separator === ";" ? "; " : " feat. ";
    credits.push({ name, join_phrase: join });
  }
  if (credits.length) credits[credits.length - 1].join_phrase = "";
  return credits.length ? credits : [{ name: "", join_phrase: "" }];
}

/** The artist line the chips stand for. */
function textFromCredits(credits: ArtistCredit[]): string {
  const named = credits.filter((credit) => credit.name.trim());
  return named
    .map((credit, index) =>
      index + 1 === named.length ? credit.name.trim() : credit.name.trim() + credit.join_phrase,
    )
    .join("");
}

/** Edits the tags by hand or from a song link. Saved by the screen's "Save tags" button. */
export function ManualEditor({
  track,
  candidate,
  onBusy,
  onError,
  onSaved,
}: {
  track: Track;
  candidate?: Candidate;
  onBusy: (saving: boolean) => void;
  onError: (message: string) => void;
  onSaved: () => Promise<void>;
}) {
  const [form, setForm] = useState<Record<string, string | number>>({
    title: candidate?.title || track.current_title || "",
    artist: candidate?.artist || track.current_artist || "",
    album: candidate?.album || track.current_album || "",
    album_artist: candidate?.album_artist || track.current_album_artist || "",
    track_number: candidate?.track_number || track.current_track_number || "",
    year: candidate?.year || "",
    release_date: candidate?.release_date || "",
    genre: candidate?.genre || "",
    composer: candidate?.composer || "",
    label: candidate?.label || "",
    isrc: candidate?.isrc || "",
    cover_url: candidate?.cover_url || "",
  });
  const [sourceUrl, setSourceUrl] = useState("");
  const [artistCredits, setArtistCredits] = useState<ArtistCredit[]>(
    candidate?.artist_credits?.length
      ? candidate.artist_credits
      : [{ name: candidate?.artist || track.current_artist || "", join_phrase: "" }],
  );
  const [albumArtistCredits, setAlbumArtistCredits] = useState<ArtistCredit[]>(
    candidate?.album_artist_credits?.length
      ? candidate.album_artist_credits
      : [{ name: candidate?.album_artist || track.current_album_artist || "", join_phrase: "" }],
  );
  const [resolving, setResolving] = useState(false);
  const [imported, setImported] = useState("");

  const resolveSource = async () => {
    setResolving(true);
    setImported("");
    try {
      const found = await post<Candidate>("/source/resolve", { url: sourceUrl });
      setForm((current) => ({
        ...current,
        title: found.title || current.title,
        artist: found.artist || current.artist,
        album: found.album || current.album,
        album_artist: found.album_artist || current.album_artist,
        track_number: found.track_number || current.track_number,
        year: found.year || current.year,
        release_date: found.release_date || current.release_date,
        genre: found.genre || current.genre,
        composer: found.composer || current.composer,
        label: found.label || current.label,
        isrc: found.isrc || current.isrc,
        cover_url: found.cover_url || current.cover_url,
      }));
      // The chips follow what was imported, so old chips cannot undo it on save.
      if (found.artist_credits?.length) setArtistCredits(found.artist_credits);
      else if (found.artist) setArtistCredits(creditsFromText(found.artist));
      if (found.album_artist_credits?.length) setAlbumArtistCredits(found.album_artist_credits);
      else if (found.album_artist) setAlbumArtistCredits(creditsFromText(found.album_artist));
      setImported(`Filled in from ${found.artist || "the link"}. Check the fields, then save.`);
    } catch (reason) {
      onError((reason as Error).message);
    } finally {
      setResolving(false);
    }
  };

  const save = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!String(form.title).trim() || !String(form.artist).trim()) {
      onError("A title and an artist are needed.");
      return;
    }
    onBusy(true);
    try {
      await api(`/tracks/${track.id}/manual`, {
        method: "PUT",
        body: JSON.stringify({
          ...form,
          // Not editable here; sent back so saving keeps them.
          track_total: candidate?.track_total ?? null,
          disc_number: candidate?.disc_number ?? null,
          disc_total: candidate?.disc_total ?? null,
          artist_credits: artistCredits.filter((credit) => credit.name.trim()),
          album_artist_credits: albumArtistCredits.filter((credit) => credit.name.trim()),
          track_number: form.track_number ? Number(form.track_number) : null,
        }),
      });
      await onSaved();
    } catch (reason) {
      onError((reason as Error).message);
    } finally {
      onBusy(false);
    }
  };

  return (
    <form id="manual-editor" onSubmit={(event) => void save(event)}>
      <Section
        title="Fill in from a link"
        footer={
          imported ||
          "Spotify, Apple Music, Deezer, Shazam, SoundCloud, Audiomack, Navahang, Genius, Radio Javan or YouTube."
        }
      >
        <label className="field" style={{ borderRadius: 0 }}>
          <Icon name="link" className="field-icon" />
          <input
            id="source-link"
            value={sourceUrl}
            onChange={(event) => setSourceUrl(event.target.value)}
            placeholder="https://…"
            inputMode="url"
            autoComplete="off"
          />
          <button
            type="button"
            className="small-button"
            disabled={!sourceUrl.trim() || resolving}
            onClick={() => void resolveSource()}
          >
            {resolving ? <Spinner size={16} /> : "Import"}
          </button>
        </label>
      </Section>

      <Section title="Tags">
        {Object.entries(form).map(([name, value]) => (
          <label className="field-row" key={name}>
            <span>{labels[name]}</span>
            <input
              id={`tag-${name}`}
              dir="auto"
              value={value}
              inputMode={name === "track_number" || name === "year" ? "numeric" : undefined}
              onChange={(event) => {
                setForm({ ...form, [name]: event.target.value });
                // The line and the chips say the same thing: typing here rewrites the chips.
                if (name === "artist") setArtistCredits(creditsFromText(event.target.value));
                if (name === "album_artist")
                  setAlbumArtistCredits(creditsFromText(event.target.value));
              }}
            />
          </label>
        ))}
      </Section>

      <Section
        title="Artists on this song"
        footer="One chip per artist. Duets and features get their own chip so each artist gets credit."
      >
        <CreditsEditor
          label="Track artists"
          credits={artistCredits}
          onChange={(credits) => {
            setArtistCredits(credits);
            setForm((current) => ({ ...current, artist: textFromCredits(credits) }));
          }}
        />
        <CreditsEditor
          label="Album artists"
          credits={albumArtistCredits}
          onChange={(credits) => {
            setAlbumArtistCredits(credits);
            setForm((current) => ({ ...current, album_artist: textFromCredits(credits) }));
          }}
        />
      </Section>
      <button type="submit" className="sr-only">
        Save tags
      </button>
    </form>
  );
}

function CreditsEditor({
  label,
  credits,
  onChange,
}: {
  label: string;
  credits: ArtistCredit[];
  onChange: (credits: ArtistCredit[]) => void;
}) {
  const update = (index: number, name: string) =>
    onChange(
      credits.map((credit, position) => (position === index ? { ...credit, name } : credit)),
    );
  const remove = (index: number) => {
    const next = credits.filter((_, position) => position !== index);
    if (next.length) next[next.length - 1] = { ...next[next.length - 1], join_phrase: "" };
    onChange(next);
  };
  const add = () => {
    const next = credits.map((credit, index) =>
      index + 1 === credits.length ? { ...credit, join_phrase: " / " } : credit,
    );
    onChange([...next, { name: "", join_phrase: "" }]);
  };
  return (
    <div className="field-row" style={{ alignItems: "start" }}>
      <span style={{ paddingTop: 8 }}>{label}</span>
      <div className="credit-chips">
        {credits.map((credit, index) => (
          <span className="credit" key={`${index}-${credit.musicbrainz_id || "manual"}`}>
            <input
              aria-label={`${label} ${index + 1}`}
              dir="auto"
              value={credit.name}
              onChange={(event) => update(index, event.target.value)}
            />
            <button
              type="button"
              aria-label={`Remove ${credit.name}`}
              onClick={() => remove(index)}
            >
              <Icon name="x" size={14} />
            </button>
          </span>
        ))}
        <button type="button" className="small-button" onClick={add}>
          <Icon name="plus" size={16} />
          Add
        </button>
      </div>
    </div>
  );
}
