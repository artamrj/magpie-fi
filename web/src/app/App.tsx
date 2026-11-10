import { useCallback, useEffect, useMemo, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { queryKeys, useSetupQuery, useStatusQuery, useTracksQuery } from "../api/queries";
import type { Setup } from "../api/types";
import { Icon, type IconName } from "./Icons";
import { AddMusic } from "./components/AddMusic";
import { CoverScreen } from "./components/CoverScreen";
import { EmptyState } from "./components/EmptyStates";
import { Library } from "./components/Library";
import { ReviewList, type Feedback } from "./components/ReviewList";
import { SongScreen } from "./components/SongScreen";
import { haptic, startTrack, startView, telegram, useTelegramBackButton } from "./telegram";
import { busyPhases, friendlyError, isProblem, isReview, preferredTrack } from "./trackUtils";

const emptySetup: Setup = {
  inbox_dir: "",
  library_dir: "",
  trash_dir: "",
  trash_days: 30,
  inbox_check_seconds: 60,
  lyrics: true,
  sidecars: false,
  rsgain: false,
  library_songs: 0,
  ytdlp: false,
  audio_format: "m4a",
  max_parallel_downloads: 2,
  telegram: { configured: false },
  sources: {},
};

type Tab = "add" | "review" | "library";
const tabs: Array<{ id: Tab; label: string; icon: IconName }> = [
  { id: "add", label: "Add", icon: "plus" },
  { id: "review", label: "Review", icon: "inbox" },
  { id: "library", label: "Library", icon: "library" },
];

type Toast = { id: number; message: string; error?: boolean; undo?: () => Promise<void> };

/** A browser window wide enough for sidebar, list and detail. Telegram never is. */
function useWide() {
  const query = "(min-width: 1024px)";
  const [wide, setWide] = useState(() => !telegram && window.matchMedia(query).matches);
  useEffect(() => {
    if (telegram) return;
    const media = window.matchMedia(query);
    const update = () => setWide(media.matches);
    media.addEventListener("change", update);
    return () => media.removeEventListener("change", update);
  }, []);
  return wide;
}

export function App() {
  const queryClient = useQueryClient();
  const setupQuery = useSetupQuery();
  const statusQuery = useStatusQuery();
  const workflow = statusQuery.data;
  const busy = workflow ? busyPhases.has(workflow.phase) : false;
  const tracksQuery = useTracksQuery(busy);
  const tracks = useMemo(() => tracksQuery.data ?? [], [tracksQuery.data]);
  const setup = setupQuery.data ?? emptySetup;
  const wide = useWide();

  const [tab, setTab] = useState<Tab>(
    startView === "review" ? "review" : tracks.length ? "review" : "add",
  );
  // A bot question links straight to its song; the song may already be decided or gone.
  const [openId, setOpenId] = useState<number | undefined>(startTrack);
  const [coverMode, setCoverMode] = useState(false);
  const [toasts, setToasts] = useState<Toast[]>([]);
  const [startDecided, setStartDecided] = useState(startView === "review");

  // First open: land on Review when something waits, else on Add.
  useEffect(() => {
    if (startDecided || tracksQuery.isPending) return;
    setStartDecided(true);
    setTab(tracks.some((track) => isReview(track) || isProblem(track)) ? "review" : "add");
  }, [startDecided, tracksQuery.isPending, tracks]);

  useEffect(() => {
    const phase = statusQuery.data?.phase;
    if (!phase) return;
    void queryClient.invalidateQueries({ queryKey: queryKeys.tracks });
  }, [statusQuery.data?.phase, queryClient]);

  const refresh = useCallback(async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: queryKeys.status }),
      queryClient.invalidateQueries({ queryKey: queryKeys.tracks }),
      queryClient.invalidateQueries({ queryKey: queryKeys.setup }),
    ]);
  }, [queryClient]);

  const push = useCallback((toast: Omit<Toast, "id">) => {
    const id = Date.now() + Math.random();
    setToasts((current) => [...current.slice(-1), { ...toast, id }]);
    window.setTimeout(
      () => setToasts((current) => current.filter((item) => item.id !== id)),
      toast.undo ? 8000 : 4000,
    );
  }, []);
  const feedback = useMemo<Feedback>(
    () => ({
      notify: (message, undo) => push({ message, undo }),
      fail: (message) => push({ message: friendlyError(message), error: true }),
      refresh,
    }),
    [push, refresh],
  );
  useEffect(() => {
    const failure = setupQuery.error ?? statusQuery.error ?? tracksQuery.error;
    if (failure) feedback.fail((failure as Error).message);
  }, [setupQuery.error, statusQuery.error, tracksQuery.error, feedback]);

  const reviewCount = tracks.filter((track) => isReview(track) || isProblem(track)).length;
  const reviewOrder = useMemo(
    () => [...tracks.filter(isReview), ...tracks.filter(isProblem)],
    [tracks],
  );
  const open = tracks.find((track) => track.id === openId);
  useEffect(() => {
    if (openId !== undefined && !tracksQuery.isPending && !open) {
      setOpenId(undefined);
      if (openId === startTrack) push({ message: "That song is already decided or gone." });
    }
  }, [openId, open, tracksQuery.isPending, push]);
  const selected = open ?? (wide && tab === "review" ? preferredTrack(tracks) : undefined);

  const goBack = useCallback(() => {
    if (coverMode) setCoverMode(false);
    else setOpenId(undefined);
  }, [coverMode]);
  const showBack = !wide && tab === "review" && (openId !== undefined || coverMode);
  useTelegramBackButton(showBack ? goBack : undefined);

  const openTrack = (trackId: number) => {
    setCoverMode(false);
    setOpenId(trackId);
    if (!wide) window.scrollTo({ top: 0 });
  };
  /** After a decision: the next song that waits, or back to the list. */
  const next = () => {
    setCoverMode(false);
    const index = reviewOrder.findIndex((track) => track.id === openId);
    const following =
      reviewOrder.find((track, position) => position > index && track.id !== openId) ??
      reviewOrder.find((track) => track.id !== openId);
    setOpenId(wide ? following?.id : undefined);
    if (!wide) window.scrollTo({ top: 0 });
  };
  /** A song opened from another tab (a request's "needs your pick"). */
  const openFromElsewhere = (trackId: number) => {
    haptic.tap();
    setTab("review");
    setCoverMode(false);
    setOpenId(trackId);
    if (!wide) window.scrollTo({ top: 0 });
  };
  const switchTab = (next: Tab) => {
    haptic.tap();
    setTab(next);
    setOpenId(undefined);
    setCoverMode(false);
  };

  const position = selected
    ? (() => {
        const index = reviewOrder.findIndex((track) => track.id === selected.id);
        return index >= 0 ? { index: index + 1, total: reviewOrder.length } : undefined;
      })()
    : undefined;

  const songOrCover = (trackId: number) => {
    const track = tracks.find((item) => item.id === trackId);
    if (!track) return null;
    return coverMode ? (
      <CoverScreen
        key={`cover-${track.id}`}
        track={track}
        onBack={() => setCoverMode(false)}
        onDone={next}
        feedback={feedback}
      />
    ) : (
      <SongScreen
        key={track.id}
        track={track}
        position={position}
        onBack={goBack}
        onNext={next}
        onCover={() => setCoverMode(true)}
        feedback={feedback}
      />
    );
  };

  const reviewList = (
    <ReviewList
      tracks={tracks}
      loading={tracksQuery.isPending}
      workflow={workflow}
      trashDays={setup.trash_days}
      selectedId={selected?.id}
      wide={wide}
      onOpen={openTrack}
      feedback={feedback}
    />
  );

  const toastList = toasts.length > 0 && (
    <div className="toasts" aria-live="polite">
      {toasts.map((toast) => (
        <div
          key={toast.id}
          className={`toast ${toast.error ? "error" : ""}`}
          role={toast.error ? "alert" : "status"}
        >
          <Icon name={toast.error ? "alert" : "check"} size={18} />
          <span>{toast.message}</span>
          {toast.undo && (
            <button
              type="button"
              onClick={() => {
                setToasts((current) => current.filter((item) => item.id !== toast.id));
                void toast.undo!().catch((reason: Error) => feedback.fail(reason.message));
              }}
            >
              Undo
            </button>
          )}
          <button
            type="button"
            aria-label="Dismiss"
            onClick={() => setToasts((current) => current.filter((item) => item.id !== toast.id))}
          >
            <Icon name="x" size={16} />
          </button>
        </div>
      ))}
    </div>
  );

  if (wide)
    return (
      <div className="app wide">
        <aside className="sidebar">
          <div className="brand">
            <i>
              <Icon name="music" size={16} />
            </i>
            magpie-fi
          </div>
          <nav aria-label="Sections">
            {tabs.map((item) => (
              <button
                type="button"
                key={item.id}
                className={tab === item.id ? "on" : ""}
                aria-current={tab === item.id ? "page" : undefined}
                onClick={() => switchTab(item.id)}
              >
                <Icon name={item.icon} size={20} />
                {item.id === "add" ? "Add music" : item.label}
                {item.id === "review" && reviewCount > 0 && (
                  <span className="badge">{reviewCount}</span>
                )}
              </button>
            ))}
          </nav>
          <div className="status">
            <i />
            {busy ? workflow?.message || "Working" : "Watching the inbox"}
            <br />
            {setup.library_songs.toLocaleString()} songs · bot{" "}
            {setup.telegram.configured ? "on" : "off"}
          </div>
        </aside>
        {tab === "review" ? (
          <>
            <section className="pane">
              <div className="screen">{reviewList}</div>
            </section>
            <aside className="detail">
              <div className="screen">
                {selected ? (
                  songOrCover(selected.id)
                ) : (
                  <EmptyState
                    icon="inbox"
                    title="Nothing to decide"
                    description="Songs that need your pick open here."
                  />
                )}
              </div>
            </aside>
          </>
        ) : tab === "add" ? (
          <AddMusic wide feedback={feedback} onOpenTrack={openFromElsewhere} />
        ) : (
          <Library setup={setup} wide feedback={feedback} />
        )}
        {toastList}
      </div>
    );

  const screen =
    tab === "review" ? (
      openId !== undefined && open ? (
        <main className="screen no-tabs">{songOrCover(open.id)}</main>
      ) : (
        <main className="screen">{reviewList}</main>
      )
    ) : tab === "add" ? (
      <AddMusic wide={false} feedback={feedback} onOpenTrack={openFromElsewhere} />
    ) : (
      <Library setup={setup} wide={false} feedback={feedback} />
    );

  return (
    <div className="app">
      {screen}
      {!showBack && (
        <nav className="tabbar" aria-label="Sections">
          {tabs.map((item) => (
            <button
              type="button"
              key={item.id}
              className={tab === item.id ? "on" : ""}
              aria-current={tab === item.id ? "page" : undefined}
              onClick={() => switchTab(item.id)}
            >
              <Icon name={item.icon} size={26} />
              {item.label}
              {item.id === "review" && reviewCount > 0 && (
                <span className="badge">{reviewCount}</span>
              )}
            </button>
          ))}
        </nav>
      )}
      {toastList}
    </div>
  );
}
