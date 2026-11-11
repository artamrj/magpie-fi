import { useState } from "react";
import { post } from "../../api/client";
import type {
  AutoApproveResult,
  RetryIssuesResult,
  Track,
  TrackCounts,
  TrackList,
  Workflow,
} from "../../api/types";
import { Icon } from "../Icons";
import { askConfirm, haptic } from "../telegram";
import { busyPhases, problemReason, reviewReason, trackArtist, trackTitle } from "../trackUtils";
import { ChosenCover, OriginalCover } from "./Cover";
import { EmptyState, ListSkeleton } from "./EmptyStates";
import { BottomActions, Cell, PageTitle, Section, Spinner, Tag } from "./ui";

export type Feedback = {
  notify: (message: string, undo?: () => Promise<void>) => void;
  fail: (message: string) => void;
  refresh: () => Promise<void>;
};

const noCounts: TrackCounts = {
  review: 0,
  issues: 0,
  skipped: 0,
  working: 0,
  ready: 0,
  confident: 0,
  damaged: 0,
};

/** Every song that waits for the owner, grouped by what has to happen to it. A long queue is
 *  shown a page at a time and searched on the server, so the page stays quick with thousands. */
export function ReviewList({
  list,
  loading,
  search,
  onSearch,
  onMore,
  workflow,
  trashDays,
  selectedId,
  wide,
  onOpen,
  feedback,
}: {
  list?: TrackList;
  loading: boolean;
  search: string;
  onSearch: (value: string) => void;
  /** Show more songs of every section. */
  onMore: () => void;
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

  const tracks = list?.tracks ?? [];
  const counts = list?.counts ?? noCounts;
  const matching = list?.matching ?? noCounts;
  const review = tracks.filter((track) => track.group === "review");
  const problems = tracks.filter((track) => track.group === "issues");
  const ready = tracks.filter((track) => track.group === "ready");
  const working = tracks.filter((track) => track.group === "working");
  const total = counts.review + counts.issues + counts.skipped + counts.working + counts.ready;
  const searching = search.trim().length > 0;
  /** "Show 100 more" under a section that has more songs than are on the page. */
  const more = (shown: number, inSection: number) =>
    inSection > shown && (
      <Cell
        key="more"
        tone="accent"
        lead={<Icon name="chevron" />}
        title={`Show ${Math.min(100, inSection - shown)} more`}
        subtitle={`${shown.toLocaleString()} of ${inSection.toLocaleString()} shown`}
        onClick={onMore}
      />
    );

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
      const damaged = counts.damaged;
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
        counts.review ? `${counts.review.toLocaleString()} need your pick` : "",
        counts.issues ? `${counts.issues} ${counts.issues === 1 ? "issue" : "issues"}` : "",
        counts.ready ? `${counts.ready.toLocaleString()} ready to file` : "",
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
          <span className="busy-text">
            {workflow.phase === "apply" ? "Filing" : "Identifying"}
            {workflow.total
              ? ` ${Math.min(Math.max(workflow.current, workflow.processed) + 1, workflow.total)} of ${workflow.total}`
              : "…"}
          </span>
          <button type="button" className="text-button" onClick={() => void stop()}>
            Stop
          </button>
          {workflow.current_file && (
            <span className="busy-file" dir="auto">
              {workflow.current_file}
            </span>
          )}
        </div>
      )}

      {(total > 8 || searching) && (
        <label className="search-field">
          <Icon name="search" size={18} />
          <span className="sr-only">Search the songs that wait</span>
          <input
            id="review-search"
            value={search}
            onChange={(event) => onSearch(event.target.value)}
            placeholder="Search by title, artist or file name"
            autoComplete="off"
          />
        </label>
      )}

      {loading ? (
        <section className="section">
          <ListSkeleton />
        </section>
      ) : total === 0 ? (
        <EmptyState
          icon="check"
          title="Nothing to review"
          description="Songs magpie-fi is unsure about appear here. Everything else is filed on its own."
        />
      ) : tracks.length === 0 ? (
        <EmptyState
          icon="search"
          title="No song fits"
          description="Nothing that waits has these words in its title, artist or file name."
          action="Clear the search"
          onAction={() => onSearch("")}
        />
      ) : (
        <>
          {review.length > 0 && (
            <Section title="Needs your pick">
              {counts.confident > 0 && !searching && (
                <Cell
                  tone="accent"
                  lead={accepting ? <Spinner /> : <Icon name="sparkles" />}
                  title={`Accept ${counts.confident} confident ${counts.confident === 1 ? "match" : "matches"}`}
                  subtitle={(list?.confident_titles ?? [])
                    .join(", ")
                    .concat(counts.confident > 3 ? "…" : "")}
                  chevron
                  disabled={accepting}
                  onClick={() => void acceptConfident()}
                />
              )}
              {review.map((track) =>
                row(
                  track,
                  reviewReason(track),
                  (track.candidate_count ?? track.candidates.length) === 0 ? (
                    <Tag>No match</Tag>
                  ) : reviewReason(track).startsWith("Maybe") ? (
                    <Tag tone="info">Duplicate?</Tag>
                  ) : reviewReason(track).includes("cover") ? (
                    <Tag tone="warn">Cover</Tag>
                  ) : undefined,
                ),
              )}
              {more(review.length, matching.review)}
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
              {more(problems.length, matching.issues + matching.skipped)}
            </Section>
          )}

          {working.length > 0 && (
            <Section title="Working on">
              {working.map((track) => row(track, "Identifying…", <Spinner />))}
              {more(working.length, matching.working)}
            </Section>
          )}

          {ready.length > 0 && (
            <Section
              title="Ready to file"
              aside={counts.ready}
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
              {more(ready.length, matching.ready)}
            </Section>
          )}
        </>
      )}

      <BottomActions
        primary={
          counts.ready > 0 && !busy
            ? {
                text: `File ${counts.ready} ready ${counts.ready === 1 ? "song" : "songs"}`,
                onClick: () => void fileReady(),
                busy: filing,
              }
            : undefined
        }
      />
    </>
  );
}
