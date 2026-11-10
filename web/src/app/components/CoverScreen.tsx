import { useEffect, useState } from "react";
import { api, post } from "../../api/client";
import type { Track } from "../../api/types";
import { Icon } from "../Icons";
import { haptic } from "../telegram";
import { candidateSources, fileStem, formatDuration, selectedCandidate } from "../trackUtils";
import { CandidateCover, ChosenCover, OriginalCover } from "./Cover";
import type { Feedback } from "./ReviewList";
import { BottomActions, Callout, Cell, Field, PageTitle, Section } from "./ui";

/** Picks the cover for a chosen match: one of the covers already found, or a pasted link. */
export function CoverScreen({
  track,
  onBack,
  onDone,
  feedback,
}: {
  track: Track;
  onBack: () => void;
  onDone: () => void;
  feedback: Feedback;
}) {
  const chosen = selectedCandidate(track);
  const options = track.candidates.filter((candidate) => candidate.cover_url);
  const [pickedUrl, setPickedUrl] = useState<string | undefined>(
    chosen?.cover_url || options[0]?.cover_url,
  );
  const [link, setLink] = useState("");
  const [busy, setBusy] = useState<"save" | "search" | "skip">();
  useEffect(() => {
    setPickedUrl(chosen?.cover_url || options[0]?.cover_url);
    setLink("");
  }, [track.id]); // eslint-disable-line react-hooks/exhaustive-deps

  const act = async (kind: NonNullable<typeof busy>, action: () => Promise<void>) => {
    setBusy(kind);
    try {
      await action();
    } catch (reason) {
      haptic.error();
      feedback.fail((reason as Error).message);
    } finally {
      setBusy(undefined);
    }
  };

  const applyCover = (url: string) =>
    act("save", async () => {
      await api(`/tracks/${track.id}/artwork`, {
        method: "PUT",
        body: JSON.stringify({ cover_url: url }),
      });
      await feedback.refresh();
      haptic.success();
      feedback.notify("Cover set");
      onDone();
    });

  const search = () =>
    act("search", async () => {
      await post(`/tracks/${track.id}/artwork/search`);
      await feedback.refresh();
      feedback.notify("Searched the catalogs again");
    });

  const title = chosen?.title || track.current_title || fileStem(track.filename);
  const verified = chosen?.artwork_status === "verified";
  const pendingLink = link.trim();

  return (
    <>
      <PageTitle title="Pick a cover" onBack={onBack} />
      <div className="hero">
        {chosen && verified ? (
          <ChosenCover track={track} candidate={chosen} size="large" />
        ) : (
          <OriginalCover track={track} size="large" />
        )}
        <div>
          <h2 dir="auto">{title}</h2>
          <p dir="auto">
            {[chosen?.artist || track.current_artist, chosen?.year, formatDuration(track.duration)]
              .filter(Boolean)
              .join(" · ")}
          </p>
        </div>
      </div>

      <Callout tone={verified ? "good" : "warn"} icon="image">
        {verified ? (
          <>
            <b>This song has a verified cover.</b> Pick another one below if you prefer it.
          </>
        ) : (
          <>
            <b>No cover matched this release.</b>{" "}
            {chosen?.artwork_message ||
              "Pick one of the covers found, or paste a link to an image or a song page."}
          </>
        )}
      </Callout>

      <Section
        title="Covers found"
        footer="Square, at least 500 px. It is embedded in the file at about 1200 px."
      >
        {options.length > 0 ? (
          <div className="cover-strip" role="radiogroup" aria-label="Covers found">
            {options.map((candidate) => (
              <button
                type="button"
                key={candidate.id}
                className={candidate.cover_url === pickedUrl && !pendingLink ? "picked" : ""}
                role="radio"
                aria-checked={candidate.cover_url === pickedUrl && !pendingLink}
                onClick={() => {
                  haptic.tap();
                  setLink("");
                  setPickedUrl(candidate.cover_url);
                }}
              >
                <CandidateCover candidate={candidate} size="medium" />
                <small>{candidateSources(candidate)}</small>
              </button>
            ))}
          </div>
        ) : (
          <Cell title="No covers found yet" subtitle="Search again or paste a link" />
        )}
        <Cell
          tone="link"
          lead={busy === "search" ? <span className="spinner" /> : <Icon name="refresh" />}
          title="Search the catalogs again"
          disabled={busy === "search"}
          onClick={() => void search()}
        />
      </Section>

      <Section
        title="Or paste a link"
        footer="An image, or a song page on Spotify, Shazam, Genius or Radio Javan."
      >
        <div style={{ padding: 6 }}>
          <Field
            icon="link"
            id="cover-link"
            value={link}
            onChange={(event) => setLink(event.target.value)}
            placeholder="https://…"
            inputMode="url"
            autoComplete="off"
          />
        </div>
      </Section>

      <BottomActions
        primary={{
          text: "Use this cover",
          onClick: () => void applyCover(pendingLink || pickedUrl || ""),
          disabled: !pendingLink && !pickedUrl,
          busy: busy === "save",
        }}
        secondary={{ text: "Later", onClick: onBack }}
      />
    </>
  );
}
