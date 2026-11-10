import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { post } from "../api/client";
import { queryKeys, useSetupQuery, useStatusQuery, useTracksQuery } from "../api/queries";
import type { AutoApproveResult, RetryIssuesResult, Setup, Track } from "../api/types";
import { Icon } from "./Icons";
import { ViewTabs } from "./components/ViewTabs";
import { TrackRow } from "./components/TrackRow";
import { TrackInspector, InspectorEmpty } from "./components/TrackInspector";
import { SettingsDrawer } from "./components/SettingsDrawer";
import { GetMusic } from "./components/GetMusic";
import { askConfirm, startView, telegram } from "./telegram";
import { ProcessingDock } from "./components/ProcessingDock";
import { EmptyState, QueueSkeleton } from "./components/EmptyStates";
import type { QueueFilter, QueueOrder, Theme } from "./trackUtils";
import {
  busyPhases,
  compareTracks,
  filterTitle,
  folderName,
  friendlyError,
  hasRetryableArtwork,
  isProblem,
  isReady,
  isReview,
  matchesFilter,
  preferredTrack,
  queueOrderLabels,
  queueOrderSequence,
  selectedCandidate,
} from "./trackUtils";

const emptySetup: Setup = {
  inbox_dir: "",
  library_dir: "",
  trash_dir: "",
  trash_days: 30,
  inbox_check_seconds: 60,
  lyrics: true,
  rsgain: false,
  library_songs: 0,
  ytdlp: false,
  audio_format: "m4a",
  max_parallel_downloads: 2,
  telegram: { configured: false },
  sources: {},
};

export function App() {
  const queryClient = useQueryClient();
  const setupQuery = useSetupQuery();
  const statusQuery = useStatusQuery();
  const workflow = statusQuery.data;
  const busy = workflow ? busyPhases.has(workflow.phase) : false;
  const tracksQuery = useTracksQuery(busy);
  const tracks = useMemo(() => tracksQuery.data ?? [], [tracksQuery.data]);
  const setup = setupQuery.data ?? emptySetup;
  const [selectedId, setSelectedId] = useState<number>();
  // Links from the bot open straight on Review (?view=review).
  const [filter, setFilter] = useState<QueueFilter>(startView === "review" ? "review" : "all");
  const [view, setView] = useState<"get" | "queue">("queue");
  const [queueOrder, setQueueOrder] = useState<QueueOrder>("queue");
  const [query, setQuery] = useState("");
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [autoApproving, setAutoApproving] = useState(false);
  const [retryingIssues, setRetryingIssues] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [mobileInspector, setMobileInspector] = useState(false);
  const [theme, setTheme] = useState<Theme>(() => {
    if (telegram) return telegram.colorScheme;
    const saved = localStorage.getItem("magpie-fi-theme") ?? localStorage.getItem("magpie-theme");
    if (saved === "light" || saved === "dark") return saved;
    return window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
  });
  const lastPhaseRef = useRef("");
  const loading = setupQuery.isPending || statusQuery.isPending || tracksQuery.isPending;
  const connected = !setupQuery.isError && !statusQuery.isError && !tracksQuery.isError;

  useEffect(() => {
    const phase = statusQuery.data?.phase;
    if (!phase || phase === lastPhaseRef.current) return;
    lastPhaseRef.current = phase;
    void queryClient.invalidateQueries({ queryKey: queryKeys.tracks });
  }, [statusQuery.data?.phase, queryClient]);

  useEffect(() => {
    const failure = setupQuery.error ?? statusQuery.error ?? tracksQuery.error;
    if (failure) setError((failure as Error).message);
  }, [setupQuery.error, statusQuery.error, tracksQuery.error]);

  const invalidateStatusAndTracks = useCallback(async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: queryKeys.status }),
      queryClient.invalidateQueries({ queryKey: queryKeys.tracks }),
    ]);
  }, [queryClient]);

  useEffect(() => {
    // Follow Telegram's light/dark switch while the Mini App is open.
    telegram?.onEvent("themeChanged", () => setTheme(telegram!.colorScheme));
  }, []);

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    localStorage.setItem("magpie-fi-theme", theme);
  }, [theme]);

  useEffect(() => {
    if (!notice) return;
    const timer = window.setTimeout(() => setNotice(""), 5000);
    return () => window.clearTimeout(timer);
  }, [notice]);

  useEffect(() => {
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      setSettingsOpen(false);
      setMobileInspector(false);
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, []);

  /** Runs a user action; a failure is shown in the error toast. */
  const run = async (action: () => Promise<void>) => {
    setError("");
    try {
      await action();
    } catch (reason) {
      setError((reason as Error).message);
    }
  };

  const checkInbox = () =>
    run(async () => {
      const result = await post<{ already_running: boolean }>("/inbox/check");
      setNotice(
        result.already_running
          ? "magpie-fi is already working on the inbox."
          : "Checking the inbox now.",
      );
      await invalidateStatusAndTracks();
    });

  const choose = (trackId: number, candidateId: number) =>
    run(async () => {
      const result = await post<{ ready: boolean }>(`/tracks/${trackId}/choose`, {
        candidate_id: candidateId,
      });
      await queryClient.invalidateQueries({ queryKey: queryKeys.tracks });
      setNotice(
        !result.ready
          ? "Match accepted, but this track still needs a verified cover before it can be cleaned."
          : "Match accepted. This track is ready to clean.",
      );
    });

  const counts = useMemo(
    () => ({
      review: tracks.filter(isReview).length,
      ready: tracks.filter(isReady).length,
      problems: tracks.filter(isProblem).length,
    }),
    [tracks],
  );

  const autoApprovable = useMemo(
    () =>
      tracks.filter(
        (track) =>
          isReview(track) &&
          !isProblem(track) &&
          !hasRetryableArtwork(track) &&
          track.candidates.length > 0,
      ).length,
    [tracks],
  );
  const retryableArtwork = useMemo(() => tracks.filter(hasRetryableArtwork).length, [tracks]);

  const autoApprove = async () => {
    setAutoApproving(true);
    setNotice("");
    await run(async () => {
      const result = await post<AutoApproveResult>("/tracks/auto-approve");
      await queryClient.invalidateQueries({ queryKey: queryKeys.tracks });
      setNotice(
        [
          `${result.approved} review ${result.approved === 1 ? "track" : "tracks"} approved.`,
          result.low_confidence ? `${result.low_confidence} left for review.` : "",
          result.unavailable ? `${result.unavailable} unavailable.` : "",
        ]
          .filter(Boolean)
          .join(" "),
      );
    });
    setAutoApproving(false);
  };

  const retryIssues = async () => {
    const damaged = tracks.filter((track) => isProblem(track) && track.status === "corrupt").length;
    if (
      damaged &&
      !(await askConfirm(
        `magpie-fi will try to salvage ${damaged} damaged ${damaged === 1 ? "file" : "files"} by skipping unreadable frames and re-encoding the valid audio. Each damaged original is kept next to the repaired file as a .magpie-fi-damaged backup. Continue?`,
      ))
    )
      return;
    setRetryingIssues(true);
    setNotice("");
    await run(async () => {
      const result = await post<RetryIssuesResult>("/tracks/retry-issues");
      await invalidateStatusAndTracks();
      if (result.started) {
        setNotice(
          [
            `Checking ${result.queued} ${result.queued === 1 ? "file" : "files"}, repairing damaged streams, and retrying identification.`,
            result.unavailable ? `${result.unavailable} still missing.` : "",
          ]
            .filter(Boolean)
            .join(" "),
        );
      } else {
        setNotice(
          result.unavailable
            ? `${result.unavailable} source ${result.unavailable === 1 ? "file is" : "files are"} still missing. Restore them to their original locations, then check again.`
            : "No issues need to be checked.",
        );
      }
    });
    setRetryingIssues(false);
  };

  const write = () =>
    run(async () => {
      const result = await post<{ outputs: number; duplicates_skipped: number }>("/write");
      setNotice(
        result.duplicates_skipped
          ? `Filing ${result.outputs} ${result.outputs === 1 ? "song" : "songs"}; ${result.duplicates_skipped} duplicate ${result.duplicates_skipped === 1 ? "copy goes" : "copies go"} to trash.`
          : `Filing ${result.outputs} ${result.outputs === 1 ? "song" : "songs"}.`,
      );
      await invalidateStatusAndTracks();
    });

  const stop = () =>
    run(async () => {
      await post("/stop");
      setNotice("Stopping safely after the current operation.");
      await invalidateStatusAndTracks();
    });

  const visibleTracks = useMemo(() => {
    const normalized = query.trim().toLocaleLowerCase();
    const filteredTracks = tracks.filter((track) => {
      if (!matchesFilter(track, filter)) return false;
      if (!normalized) return true;
      const candidate = selectedCandidate(track) ?? track.candidates[0];
      return [
        track.filename,
        track.current_title,
        track.current_artist,
        track.current_album,
        candidate?.title,
        candidate?.artist,
        candidate?.album,
      ].some((value) => value?.toLocaleLowerCase().includes(normalized));
    });
    if (queueOrder === "queue") return filteredTracks;
    return [...filteredTracks].sort((first, second) => compareTracks(first, second, queueOrder));
  }, [filter, query, queueOrder, tracks]);

  const cycleQueueOrder = () => {
    const currentIndex = queueOrderSequence.indexOf(queueOrder);
    setQueueOrder(queueOrderSequence[(currentIndex + 1) % queueOrderSequence.length]);
  };

  const selected = tracks.find((track) => track.id === selectedId);

  useEffect(() => {
    if (selectedId === undefined) {
      setSelectedId(preferredTrack(visibleTracks)?.id);
      return;
    }
    if (visibleTracks.some((track) => track.id === selectedId)) return;
    setSelectedId(visibleTracks[0]?.id);
  }, [selectedId, visibleTracks]);

  const changeFilter = (next: QueueFilter) => {
    setView("queue");
    setFilter(next);
    const nextTrack =
      next === "all" ? preferredTrack(tracks) : tracks.find((track) => matchesFilter(track, next));
    setSelectedId(nextTrack?.id);
  };

  const tabs = {
    view,
    filter,
    counts,
    total: tracks.length,
    onGet: () => setView("get"),
    onFilter: changeFilter,
  };
  // Review offers to retry covers that failed to download; Issues offers to check and fix.
  const retryCount =
    filter === "review" ? retryableArtwork : filter === "problems" ? counts.problems : 0;

  const selectTrack = useCallback((track: Track) => {
    setSelectedId(track.id);
    setMobileInspector(true);
  }, []);

  return (
    <div className="studio-app">
      <header className="topbar">
        <div className="topbar-main">
          <div className="topbar-leading">
            <a className="brand" href="#workspace" aria-label="magpie-fi home">
              <span className="brand-mark">
                <Icon name="waveform" size={18} />
              </span>
              <span>magpie-fi</span>
            </a>
            <button
              className="topbar-source-summary"
              onClick={() => setSettingsOpen(true)}
              aria-label="Show status and folders"
              title={setup.inbox_dir}
            >
              <span className="source-icon">
                <Icon name="folder" size={16} />
              </span>
              <span className="source-copy">
                <b>{folderName(setup.inbox_dir) || "inbox"}</b>
                <small>
                  {tracks.length} waiting · {setup.library_songs} in library
                </small>
              </span>
            </button>
          </div>
          <ViewTabs className="workspace-nav" {...tabs} />
          <div className="topbar-controls">
            <button
              className={`topbar-scan-action ${tracks.length ? "" : "prominent"}`.trim()}
              disabled={busy}
              onClick={checkInbox}
              title={
                busy
                  ? workflow?.phase === "apply"
                    ? "Filing songs into the library"
                    : "Identifying music"
                  : "Look for new music in the inbox now"
              }
            >
              {busy ? <span className="spinner" /> : <Icon name="refresh" />}
              {busy ? (workflow?.phase === "apply" ? "Filing…" : "Identifying…") : "Check inbox"}
            </button>
            <div className="topbar-actions">
              {!connected && (
                <span className="connection-state offline">
                  <i />
                  Offline
                </span>
              )}
              <button
                className="icon-button"
                onClick={() => setTheme(theme === "dark" ? "light" : "dark")}
                aria-label={`Use ${theme === "dark" ? "light" : "dark"} theme`}
                title="Change theme"
              >
                <Icon name={theme === "dark" ? "sun" : "moon"} />
              </button>
              <button
                className="icon-button"
                onClick={() => setSettingsOpen(true)}
                aria-label="Open settings"
                title="Settings"
              >
                <Icon name="settings" />
              </button>
            </div>
          </div>
        </div>
      </header>

      <ViewTabs className="mobile-nav" {...tabs} />

      <div className="announcement-region" aria-live="polite" aria-atomic="true">
        {error && (
          <div className="toast error-toast" role="alert">
            <Icon name="alert" />
            <span>
              <b>{connected ? "Couldn’t complete that action" : "Backend unavailable"}</b>
              <small>{friendlyError(error)}</small>
            </span>
            <button onClick={() => setError("")} aria-label="Dismiss error">
              <Icon name="x" size={16} />
            </button>
          </div>
        )}
        {notice && (
          <div className="toast notice-toast">
            <Icon name="check" />
            <span>
              <b>Done</b>
              <small>{notice}</small>
            </span>
            <button onClick={() => setNotice("")} aria-label="Dismiss notification">
              <Icon name="x" size={16} />
            </button>
          </div>
        )}
      </div>

      {view === "get" ? (
        <GetMusic onError={setError} />
      ) : (
        <main className="workspace" id="workspace">
          <section className="queue-panel" aria-labelledby="queue-title">
            <header className="queue-header">
              <div className="queue-heading">
                <h1 id="queue-title">{filterTitle(filter)}</h1>
                <span className="queue-count">{visibleTracks.length}</span>
              </div>
              <label className="search-field">
                <Icon name="search" size={16} />
                <span className="sr-only">Search queue</span>
                <input
                  value={query}
                  onChange={(event) => setQuery(event.target.value)}
                  placeholder="Search title, artist, album…"
                />
              </label>
              <button
                className="queue-order-button compact-button"
                onClick={cycleQueueOrder}
                aria-label={`Order tracks by ${queueOrderLabels[queueOrder]}`}
                title={`Current order: ${queueOrderLabels[queueOrder]}. Click to change.`}
              >
                <Icon name="menu" size={14} />
                <span>{queueOrderLabels[queueOrder]}</span>
              </button>
              {autoApprovable > 0 && filter === "review" && (
                <button
                  className="compact-button accent"
                  disabled={autoApproving}
                  onClick={autoApprove}
                >
                  <Icon name="sparkles" size={15} />
                  {autoApproving ? "Checking…" : `Auto-select ${autoApprovable}`}
                </button>
              )}
              {retryCount > 0 && (
                <button
                  className="compact-button accent"
                  disabled={busy || retryingIssues}
                  onClick={() => void retryIssues()}
                >
                  {retryingIssues || busy ? (
                    <span className="spinner" />
                  ) : (
                    <Icon name="refresh" size={15} />
                  )}
                  {retryingIssues || busy
                    ? filter === "review"
                      ? "Retrying…"
                      : "Checking…"
                    : filter === "review"
                      ? `Retry ${retryCount} ${retryCount === 1 ? "cover" : "covers"}`
                      : `Check & fix ${retryCount}`}
                </button>
              )}
            </header>

            <div className="track-list" role="list" aria-label="Music queue">
              {loading ? (
                <QueueSkeleton />
              ) : !connected && tracks.length === 0 ? (
                <EmptyState
                  icon="alert"
                  title="The studio is offline"
                  description="Start the local Rust backend, then reconnect to load your workspace."
                  action="Reconnect"
                  onAction={() => void invalidateStatusAndTracks()}
                />
              ) : tracks.length === 0 ? (
                <EmptyState
                  icon={workflow?.phase === "finish" ? "check" : "music"}
                  title={
                    busy
                      ? "Listening to new music"
                      : workflow?.phase === "finish"
                        ? "Everything is filed"
                        : "Nothing waiting"
                  }
                  description={
                    busy
                      ? "Audio files will appear here as they are discovered and identified."
                      : workflow?.phase === "finish"
                        ? `New songs are in your library: ${setup.library_dir}.`
                        : "Drop music into the inbox folder. magpie-fi checks it every minute."
                  }
                />
              ) : visibleTracks.length === 0 ? (
                <EmptyState
                  icon={filter === "review" || filter === "ready" ? "check" : "search"}
                  title={
                    filter === "review"
                      ? "Nothing needs review"
                      : filter === "ready"
                        ? "No tracks are ready yet"
                        : "No tracks found"
                  }
                  description={
                    filter === "review"
                      ? "All current matches are resolved. Your library is ready for the next step."
                      : filter === "ready"
                        ? "Accept a match or enter metadata manually to prepare a track for writing."
                        : "Try another search or switch workspace views."
                  }
                />
              ) : (
                visibleTracks.map((track) => (
                  <TrackRow
                    key={track.id}
                    track={track}
                    active={track.id === selectedId}
                    onSelect={selectTrack}
                  />
                ))
              )}
            </div>
          </section>

          <aside
            className={`inspector-panel ${mobileInspector ? "mobile-open" : ""}`}
            aria-label="Metadata inspector"
          >
            <button
              className="mobile-close icon-button"
              onClick={() => setMobileInspector(false)}
              aria-label="Close inspector"
            >
              <Icon name="x" />
            </button>
            {selected ? (
              <TrackInspector
                track={selected}
                onChoose={choose}
                onSaved={() => queryClient.invalidateQueries({ queryKey: queryKeys.tracks })}
              />
            ) : (
              <InspectorEmpty busy={busy} />
            )}
          </aside>
        </main>
      )}

      <ProcessingDock
        workflow={workflow}
        counts={counts}
        busy={busy}
        trashDays={setup.trash_days}
        onStop={() => void stop()}
        onWrite={() => void write()}
      />

      {settingsOpen && <SettingsDrawer setup={setup} onClose={() => setSettingsOpen(false)} />}
    </div>
  );
}
