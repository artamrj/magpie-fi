import { useEffect, useRef, useState } from "react";
import { api, post } from "../../api/client";
import type { Candidate, Track } from "../../api/types";
import { Icon } from "../Icons";
import { askConfirm, haptic, hasTelegramButtons } from "../telegram";
import {
  reviewExplanation,
  candidateChips,
  candidateSignals,
  candidateSources,
  changedFields,
  fileStem,
  formatDuration,
  isPossibleDuplicate,
  isProblem,
  isReady,
  isReview,
  needsCover,
  outputFilename,
  problemReason,
  selectedCandidate,
} from "../trackUtils";
import { CandidateCover, ChosenCover, OriginalCover } from "./Cover";
import { ManualEditor } from "./ManualEditor";
import type { Feedback } from "./ReviewList";
import { BottomActions, Callout, Cell, PageTitle, Section } from "./ui";

/** One song, one question: which release, file it anyway, repair it, or remove it. */
export function SongScreen({
  track,
  position,
  onBack,
  onNext,
  onCover,
  feedback,
}: {
  track: Track;
  position?: { index: number; total: number };
  onBack: () => void;
  onNext: () => void;
  onCover: () => void;
  feedback: Feedback;
}) {
  const chosen = selectedCandidate(track);
  const [pickedId, setPickedId] = useState<number | undefined>(
    chosen?.id ?? track.candidates[0]?.id,
  );
  const [editing, setEditing] = useState(false);
  const [busy, setBusy] = useState<"choose" | "file" | "remove" | "undo" | "fix" | "save">();
  useEffect(() => {
    setPickedId(chosen?.id ?? track.candidates[0]?.id);
    setEditing(false);
    setBusy(undefined);
  }, [track.id, chosen?.id, track.candidates]);

  const review = isReview(track);
  const problem = isProblem(track);
  const ready = isReady(track);
  const duplicate = isPossibleDuplicate(track);
  const coverNeeded = review && needsCover(track);
  const picked = track.candidates.find((item) => item.id === pickedId);
  const shown = chosen ?? picked;

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

  const returnToReview = async () => {
    await post(`/tracks/${track.id}/review`);
    await feedback.refresh();
  };

  const choose = () =>
    act("choose", async () => {
      if (!picked) return;
      const result = await post<{ ready: boolean; cover_verified: boolean }>(
        `/tracks/${track.id}/choose`,
        { candidate_id: picked.id },
      );
      await feedback.refresh();
      haptic.success();
      if (result.ready) {
        feedback.notify("Match accepted", returnToReview);
        onNext();
      } else if (!result.cover_verified) {
        feedback.notify("Match accepted. It still needs a cover.");
        onCover();
      } else {
        feedback.notify(
          "Match accepted, but some details are missing. Edit the tags to fill them.",
        );
      }
    });

  const fileAnyway = () =>
    act("file", async () => {
      await post(`/tracks/${track.id}/file-anyway`);
      await feedback.refresh();
      feedback.notify("It will be filed as a new song");
      onNext();
    });

  const remove = () =>
    act("remove", async () => {
      if (
        !(await askConfirm(
          `Remove “${track.filename}”? Your own files go to the trash for a while; downloads are deleted.`,
        ))
      )
        return;
      await api(`/tracks/${track.id}`, { method: "DELETE" });
      await feedback.refresh();
      feedback.notify("Removed");
      onNext();
    });

  const undo = () =>
    act("undo", async () => {
      await returnToReview();
      feedback.notify("Back in Review");
    });

  const fix = () =>
    act("fix", async () => {
      await post("/tracks/retry-issues");
      await feedback.refresh();
      feedback.notify("Checking the file again");
      onBack();
    });

  const actions = (() => {
    if (editing)
      return {
        primary: {
          text: "Save tags",
          onClick: () => document.querySelector<HTMLFormElement>("#manual-editor")?.requestSubmit(),
          busy: busy === "save",
        },
        secondary: { text: "Cancel", onClick: () => setEditing(false) },
      };
    if (problem && track.stage !== "skipped")
      return {
        primary: { text: "Check and repair", onClick: () => void fix(), busy: busy === "fix" },
        secondary: {
          text: "Remove",
          onClick: () => void remove(),
          busy: busy === "remove",
          destructive: true,
        },
      };
    if (track.stage === "skipped")
      return {
        primary: {
          text: "Remove",
          onClick: () => void remove(),
          busy: busy === "remove",
          destructive: true,
        },
      };
    if (duplicate)
      return {
        primary: { text: "File anyway", onClick: () => void fileAnyway(), busy: busy === "file" },
        secondary: {
          text: "Remove",
          onClick: () => void remove(),
          busy: busy === "remove",
          destructive: true,
        },
      };
    if (coverNeeded) return { primary: { text: "Pick a cover", onClick: onCover } };
    if (review && track.candidates.length > 0)
      return {
        primary: {
          text: "Use this match",
          onClick: () => void choose(),
          busy: busy === "choose",
          disabled: !picked,
        },
        secondary: { text: "Skip", onClick: onNext },
      };
    if (review)
      return {
        primary: { text: "Edit the tags myself", onClick: () => setEditing(true) },
        secondary: {
          text: "Remove",
          onClick: () => void remove(),
          busy: busy === "remove",
          destructive: true,
        },
      };
    return {};
  })();

  // In a browser window: Enter runs the main action, → goes to the next song.
  useEffect(() => {
    if (hasTelegramButtons || editing) return;
    const onKey = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      if (target && /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName)) return;
      if (event.key === "Enter" && actions.primary && !actions.primary.disabled) {
        event.preventDefault();
        actions.primary.onClick();
      } else if (event.key === "ArrowRight" && review) {
        event.preventDefault();
        onNext();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const title = editing
    ? "Edit tags"
    : problem
      ? "Issue"
      : ready
        ? "Ready to file"
        : position
          ? `Song ${position.index} of ${position.total}`
          : "Song";

  return (
    <>
      <PageTitle title={title} onBack={onBack} />
      <Hero track={track} candidate={chosen} />

      {editing ? (
        <ManualEditor
          track={track}
          candidate={shown}
          onBusy={(saving) => setBusy(saving ? "save" : undefined)}
          onError={feedback.fail}
          onSaved={async () => {
            await feedback.refresh();
            setEditing(false);
            feedback.notify("Tags saved");
          }}
        />
      ) : (
        <>
          <Explanation track={track} duplicate={duplicate} coverNeeded={coverNeeded} />

          {review && !duplicate && !coverNeeded && track.candidates.length > 0 && (
            <Section title="Which release?">
              {track.candidates.slice(0, 5).map((candidate, index) => (
                <ReleaseOption
                  key={candidate.id}
                  track={track}
                  candidate={candidate}
                  rank={index + 1}
                  picked={candidate.id === pickedId}
                  onPick={() => {
                    haptic.tap();
                    setPickedId(candidate.id);
                  }}
                />
              ))}
            </Section>
          )}

          {shown && !duplicate && <Changes track={track} candidate={shown} />}

          <Section title="More">
            {!problem && (
              <Cell
                tone="link"
                lead={<Icon name="edit" />}
                title="Edit the tags myself"
                chevron
                onClick={() => setEditing(true)}
              />
            )}
            {chosen && !coverNeeded && (
              <Cell
                tone="link"
                lead={<Icon name="image" />}
                title="Choose a different cover"
                chevron
                onClick={onCover}
              />
            )}
            {ready && (
              <Cell
                tone="link"
                lead={<Icon name="refresh" />}
                title="Put it back in Review"
                subtitle="Undo the match and choose again"
                disabled={busy === "undo"}
                onClick={() => void undo()}
              />
            )}
            {review && track.candidates.length > 0 && !coverNeeded && (
              <Cell tone="link" lead={<Icon name="skip" />} title="Skip for now" onClick={onNext} />
            )}
            {track.error && <Cell title="Technical details" subtitle={track.error} />}
            <Cell
              tone="danger"
              lead={<Icon name="trash" />}
              title="Remove this file"
              disabled={busy === "remove"}
              onClick={() => void remove()}
            />
          </Section>
        </>
      )}

      <BottomActions {...actions} />
    </>
  );
}

function Hero({ track, candidate }: { track: Track; candidate?: Candidate }) {
  const playable = !track.is_missing && track.status !== "corrupt" && track.stage !== "skipped";
  const title = candidate?.title || track.current_title || fileStem(track.filename);
  const artist = candidate?.artist || track.current_artist;
  const facts = [track.format?.toUpperCase(), formatDuration(track.duration)].filter(Boolean);
  return (
    <div className="hero">
      {candidate ? (
        <ChosenCover track={track} candidate={candidate} size="large" />
      ) : (
        <OriginalCover track={track} size="large" />
      )}
      <div className="hero-main">
        <h2 dir="auto">{title}</h2>
        {artist && <p dir="auto">{artist}</p>}
        {playable ? (
          <Player key={track.id} src={`/api/tracks/${track.id}/audio`} duration={track.duration} />
        ) : (
          facts.length > 0 && <p>{facts.join(" · ")}</p>
        )}
        {playable && facts.length > 0 && <p className="hero-facts">{facts.join(" · ")}</p>}
      </div>
    </div>
  );
}

/** Play, pause and seek anywhere in the song. The audio loads on the first play only. */
export function Player({ src, duration }: { src: string; duration?: number }) {
  const audioRef = useRef<HTMLAudioElement>(null);
  const [playing, setPlaying] = useState(false);
  const [time, setTime] = useState(0);
  const [length, setLength] = useState(duration ?? 0);
  const [scrubbing, setScrubbing] = useState<number>();
  useEffect(() => () => audioRef.current?.pause(), []);

  const toggle = () => {
    const audio = audioRef.current;
    if (!audio) return;
    if (!audio.src) audio.src = src;
    if (audio.paused) void audio.play().catch(() => setPlaying(false));
    else audio.pause();
  };
  const seek = (seconds: number) => {
    const audio = audioRef.current;
    if (!audio) return;
    if (!audio.src) audio.src = src;
    audio.currentTime = seconds;
    setTime(seconds);
  };
  const shown = scrubbing ?? time;
  return (
    <div className="player">
      <audio
        ref={audioRef}
        preload="none"
        onPlay={() => setPlaying(true)}
        onPause={() => setPlaying(false)}
        onEnded={() => setPlaying(false)}
        onTimeUpdate={(event) => {
          if (scrubbing === undefined) setTime(event.currentTarget.currentTime);
        }}
        onLoadedMetadata={(event) => {
          if (Number.isFinite(event.currentTarget.duration))
            setLength(event.currentTarget.duration);
        }}
      />
      <button
        type="button"
        className="player-toggle"
        onClick={toggle}
        aria-label={playing ? "Pause" : "Play"}
      >
        <Icon name={playing ? "pause" : "play"} size={16} />
      </button>
      <time className="player-time">{formatDuration(shown) ?? "0:00"}</time>
      <input
        type="range"
        className="player-seek"
        min={0}
        max={length || 1}
        step={0.5}
        value={Math.min(shown, length || 1)}
        aria-label="Position in the song"
        disabled={!length}
        onChange={(event) => setScrubbing(Number(event.target.value))}
        onPointerUp={(event) => {
          const value = Number((event.target as HTMLInputElement).value);
          setScrubbing(undefined);
          seek(value);
        }}
        onKeyUp={(event) => {
          const value = Number((event.target as HTMLInputElement).value);
          setScrubbing(undefined);
          seek(value);
        }}
      />
      <time className="player-time">{formatDuration(length) ?? "–:––"}</time>
    </div>
  );
}

function Explanation({
  track,
  duplicate,
  coverNeeded,
}: {
  track: Track;
  duplicate: boolean;
  coverNeeded: boolean;
}) {
  if (isProblem(track) || track.stage === "skipped")
    return (
      <Callout tone="bad" icon="alert">
        <b>{problemReason(track)}.</b>{" "}
        {track.status === "corrupt"
          ? "The audio could not be decoded. Repair skips the broken parts; the original is kept."
          : track.is_missing
            ? "The file is no longer where it was. Put it back, then check again."
            : track.stage === "skipped"
              ? "The same file was already sent. Nothing to do."
              : track.stage_message || "Check again, or remove it."}
      </Callout>
    );
  if (duplicate)
    return (
      <Callout tone="info">
        <b>Maybe already in your library.</b>{" "}
        {track.stage_message?.replace(
          /^Possible duplicate: you may already have this song as /,
          "Looks like ",
        )}
        . A different version? File it anyway. The same song? Remove it.
      </Callout>
    );
  if (coverNeeded)
    return (
      <Callout tone="warn" icon="image">
        <b>The match is right, but no cover matched this release.</b> Pick one of the covers found,
        or paste a link.
      </Callout>
    );
  if (isReady(track))
    return (
      <Callout tone="good" icon="check">
        <b>Ready.</b> It is filed with the next batch. The cover and lyrics go inside the file.
      </Callout>
    );
  if (track.candidates.length === 0)
    return (
      <Callout tone="warn">
        <b>Nothing recognized this file.</b> Enter the title and artist yourself, or paste a link to
        the song on Spotify, Shazam, Radio Javan or YouTube.
      </Callout>
    );
  const why = reviewExplanation(track);
  if (track.candidates.length === 1)
    return (
      <Callout>
        <b>One match, but not sure enough.</b>{" "}
        {why ?? "Check the album and year, then use it or edit the tags."}
      </Callout>
    );
  return (
    <Callout>
      <b>Not sure which one this is.</b> {why ? `${why} ` : ""}Pick the right one; the first is the
      best guess.
    </Callout>
  );
}

function ReleaseOption({
  track,
  candidate,
  rank,
  picked,
  onPick,
}: {
  track: Track;
  candidate: Candidate;
  rank: number;
  picked: boolean;
  onPick: () => void;
}) {
  const chips = candidateChips(track, candidate, rank);
  const meta = [
    candidate.album,
    candidate.year || candidate.release_date?.slice(0, 4),
    candidate.track_number
      ? `track ${candidate.track_number}${candidate.track_total ? ` of ${candidate.track_total}` : ""}`
      : "",
  ]
    .filter(Boolean)
    .join(" · ");
  const signals = candidateSignals(track, candidate);
  return (
    <div className="cell has-lead option" role="radio" aria-checked={picked}>
      <button
        type="button"
        className="cell-lead"
        onClick={onPick}
        aria-label={`Use ${candidate.title}`}
      >
        <span className={`radio ${picked ? "on" : ""}`} />
        <CandidateCover candidate={candidate} size="medium" />
      </button>
      <button type="button" className="cell-main" onClick={onPick}>
        <b dir="auto">{candidate.title || "Untitled"}</b>
        {meta && <small dir="auto">{meta}</small>}
        <span className="chips">
          {chips.map((chip) => (
            <span key={chip.text} className={chip.tone === "plain" ? "" : `chip-${chip.tone}`}>
              {chip.text}
            </span>
          ))}
        </span>
      </button>
      <details className="option-details">
        <summary>Details</summary>
        <div className="compare">
          {[
            ["Title", track.current_title, candidate.title, signals[0].value],
            ["Artist", track.current_artist, candidate.artist, signals[1].value],
            ["Album", track.current_album, candidate.album, signals[2].value],
            ["Length", undefined, formatDuration(track.duration), signals[3].value],
          ].map(([label, from, to, verdict]) => (
            <div className="compare-row" key={label}>
              <small>{label}</small>
              <span dir="auto">
                {from && to && from !== to ? <span className="from">{from}</span> : null}
                <span className={to ? "to" : "empty"}>{to || "not set"}</span>
                {verdict && verdict !== "No original" ? (
                  <span className="verdict">{verdict}</span>
                ) : null}
              </span>
            </div>
          ))}
          <div className="compare-row">
            <small>Source</small>
            <span>
              {candidateSources(candidate)}
              {candidate.isrc ? ` · ISRC ${candidate.isrc}` : ""}
              {candidate.label ? ` · ${candidate.label}` : ""}
              {candidate.release_date ? ` · ${candidate.release_date}` : ""}
              {` · ${Math.round(candidate.score)} % confidence`}
            </span>
          </div>
        </div>
      </details>
    </div>
  );
}

function Changes({ track, candidate }: { track: Track; candidate: Candidate }) {
  const rows = changedFields(track, candidate);
  const filename = outputFilename(track, candidate);
  return (
    <Section
      title="What changes in your file"
      footer="Fields not listed stay as they are. The cover and lyrics are embedded in the file."
    >
      {rows.length === 0 && (
        <Cell title="The tags already match" subtitle="Only the cover and lyrics are added" />
      )}
      {rows.map((row) => (
        <div className="change-row" key={row.label}>
          <small>{row.label}</small>
          <span dir="auto">
            <span className="from">{row.from || "not set"}</span>
            <span className="to">{row.to}</span>
          </span>
        </div>
      ))}
      {filename !== track.filename && (
        <div className="change-row">
          <small>Saved as</small>
          <span dir="auto">{filename}</span>
        </div>
      )}
    </Section>
  );
}
