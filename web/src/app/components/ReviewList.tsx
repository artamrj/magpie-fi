import { useState } from "react";
import { post } from "../../api/client";
import type { AutoApproveResult, RetryIssuesResult, Track, Workflow } from "../../api/types";
import { Icon } from "../Icons";
import { askConfirm, haptic } from "../telegram";
import {
  busyPhases,
  isConfident,
  isProblem,
  isReady,
  isReview,
  problemReason,
  reviewReason,
  trackArtist,
  trackTitle,
} from "../trackUtils";
import { ChosenCover, OriginalCover } from "./Cover";
import { EmptyState, ListSkeleton } from "./EmptyStates";
import { BottomActions, Cell, PageTitle, Section, Spinner, Tag } from "./ui";

export type Feedback = {
  notify: (message: string, undo?: () => Promise<void>) => void;
  fail: (message: string) => void;
  refresh: () => Promise<void>;
};

/** Every song that waits for the owner, grouped by what has to happen to it. */
export function ReviewList({
  tracks,
  loading,
  workflow,
  trashDays,
  selectedId,
  wide,
  onOpen,
  feedback,
}: {
  tracks: Track[];
  loading: boolean;
  workflow?: Workflow;
  trashDays: number;
  selectedId?: number;
  wide: boolean;
  onOpen: (trackId: number) => void;
  feedback: Feedback;
}) {
  const [accepting, setAccepting] = useState(false);
  const [fixing, setFixing] = useState(false);
  const [filing, setFiling] = useState(false);
  const busy = workflow ? busyPhases.has(workflow.phase) : false;

  const review = tracks.filter(isReview);
  const problems = tracks.filter((track) => isProblem(track) || track.stage === "skipped");
  const ready = tracks.filter(isReady);
  const working = tracks.filter(
    (track) =>
      !isReview(track) && !isProblem(track) && !isReady(track) && track.stage !== "skipped",
  );
  const confident = review.filter(isConfident);

  const run = async (setBusy: (value: boolean) => void, action: () => Promise<void>) => {
    setBusy(true);
    try {
      await action();
      await feedback.refresh();
    } catch (reason) {
      haptic.error();
      feedback.fail((reason as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const acceptConfident = () =>
    run(setAccepting, async () => {
      const result = await post<AutoApproveResult>("/tracks/auto-approve");
      haptic.success();
      feedback.notify(
        `${result.approved} ${result.approved === 1 ? "match" : "matches"} accepted` +
          (result.low_confidence ? `, ${result.low_confidence} still need your pick` : ""),
      );
    });

  const fixIssues = () =>
    run(setFixing, async () => {
      const damaged = problems.filter((track) => track.status === "corrupt").length;
      if (
        damaged &&
        !(await askConfirm(
          `Try to repair ${damaged} damaged ${damaged === 1 ? "file" : "files"}? The unreadable parts are skipped and the rest re-encoded. Each original is kept next to the repaired file.`,
        ))
      )
        return;
      const result = await post<RetryIssuesResult>("/tracks/retry-issues");
      feedback.notify(
        result.started
          ? `Checking ${result.queued} ${result.queued === 1 ? "file" : "files"} again`
          : result.unavailable
            ? `${result.unavailable} ${result.unavailable === 1 ? "file is" : "files are"} still missing. Put them back, then try again.`
            : "Nothing to check.",
      );
    });

  const fileReady = () =>
    run(setFiling, async () => {
      const result = await post<{ outputs: number; duplicates_skipped: number }>("/write");
      haptic.success();
      feedback.notify(
        `Filing ${result.outputs} ${result.outputs === 1 ? "song" : "songs"}` +
          (result.duplicates_skipped
            ? `; ${result.duplicates_skipped} duplicate ${result.duplicates_skipped === 1 ? "copy goes" : "copies go"} to the trash`
            : ""),
      );
    });

  const stop = () =>
    run(
      () => undefined,
      async () => {
        await post("/stop");
        feedback.notify("Stopping after the current song");
      },
    );

  const subtitle = loading
    ? "Loading…"
    : [
        review.length ? `${review.length} need your pick` : "",
        problems.length ? `${problems.length} ${problems.length === 1 ? "issue" : "issues"}` : "",
        ready.length ? `${ready.length} ready to file` : "",
      ]
        .filter(Boolean)
        .join(" · ") || "Nothing is waiting";

  const row = (track: Track, sub: string, trailing?: React.ReactNode) => {
    const chosen = track.candidates.find((item) => item.id === track.selected_candidate_id);
    return (
      <Cell
        key={track.id}
        lead={
          chosen ? (
            <ChosenCover track={track} candidate={chosen} size="small" />
          ) : (
            <OriginalCover track={track} size="small" />
          )
        }
        title={trackTitle(track)}
        subtitle={[trackArtist(track), sub].filter(Boolean).join(" · ")}
        trailing={trailing}
        chevron
        selected={wide && track.id === selectedId}
        onClick={() => {
          haptic.tap();
          onOpen(track.id);
        }}
      />
    );
  };

  return (
    <>
      <PageTitle title="Review" subtitle={subtitle} />

      {busy && workflow && (
        <div className="busy-strip" role="status">
          <Spinner />
          <span>
            {workflow.phase === "apply" ? "Filing" : "Identifying"}
            {workflow.total
              ? ` ${Math.min(workflow.current || workflow.processed, workflow.total)} of ${workflow.total}`
              : ""}
            {workflow.current_file ? ` · ${workflow.current_file}` : ""}
          </span>
          <progress
            max={workflow.total || 1}
            value={Math.max(workflow.processed, workflow.current)}
            aria-label="Progress"
          />
          <button type="button" className="text-button" onClick={() => void stop()}>
            Stop
          </button>
        </div>
      )}

      {loading ? (
        <section className="section">
          <ListSkeleton />
        </section>
      ) : tracks.length === 0 ? (
        <EmptyState
          icon="check"
          title="Nothing to review"
          description="Songs magpie-fi is unsure about appear here. Everything else is filed on its own."
        />
      ) : (
        <>
          {review.length > 0 && (
            <Section title="Needs your pick">
              {confident.length > 0 && (
                <Cell
                  tone="accent"
                  lead={accepting ? <Spinner /> : <Icon name="sparkles" />}
                  title={`Accept ${confident.length} confident ${confident.length === 1 ? "match" : "matches"}`}
                  subtitle={confident
                    .slice(0, 3)
                    .map(trackTitle)
                    .join(", ")
                    .concat(confident.length > 3 ? "…" : "")}
                  chevron
                  disabled={accepting}
                  onClick={() => void acceptConfident()}
                />
              )}
              {review.map((track) =>
                row(
                  track,
                  reviewReason(track),
                  track.candidates.length === 0 ? (
                    <Tag>No match</Tag>
                  ) : reviewReason(track).startsWith("Maybe") ? (
                    <Tag tone="info">Duplicate?</Tag>
                  ) : reviewReason(track).includes("cover") ? (
                    <Tag tone="warn">Cover</Tag>
                  ) : undefined,
                ),
              )}
            </Section>
          )}

          {problems.length > 0 && (
            <Section title="Issues">
              {problems.some((track) => track.stage !== "skipped") && (
                <Cell
                  tone="accent"
                  lead={fixing ? <Spinner /> : <Icon name="refresh" />}
                  title="Check and repair"
                  subtitle="Repairs damaged files and looks for missing ones"
                  chevron
                  disabled={fixing || busy}
                  onClick={() => void fixIssues()}
                />
              )}
              {problems.map((track) =>
                row(
                  track,
                  problemReason(track),
                  <Tag tone={track.stage === "skipped" ? "gray" : "bad"}>
                    {track.status === "corrupt"
                      ? "Damaged"
                      : track.is_missing
                        ? "Missing"
                        : track.stage === "skipped"
                          ? "Skipped"
                          : "Failed"}
                  </Tag>,
                ),
              )}
            </Section>
          )}

          {working.length > 0 && (
            <Section title="Working on">
              {working.map((track) => row(track, "Identifying…", <Spinner />))}
            </Section>
          )}

          {ready.length > 0 && (
            <Section
              title="Ready to file"
              aside={ready.length}
              footer={`Filed songs show up in Navidrome within a minute. Originals stay in the trash for ${trashDays} days.`}
            >
              {ready.map((track) =>
                row(
                  track,
                  track.candidates.find((item) => item.id === track.selected_candidate_id)?.album ||
                    "",
                  <Tag tone="good">
                    <Icon name="check" size={12} />
                  </Tag>,
                ),
              )}
            </Section>
          )}
        </>
      )}

      <BottomActions
        primary={
          ready.length > 0 && !busy
            ? {
                text: `File ${ready.length} ready ${ready.length === 1 ? "song" : "songs"}`,
                onClick: () => void fileReady(),
                busy: filing,
              }
            : undefined
        }
      />
    </>
  );
}
