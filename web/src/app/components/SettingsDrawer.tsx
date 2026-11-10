import { useQuery } from "@tanstack/react-query";
import { api } from "@/api/client";
import type { Me, Setup } from "@/api/types";
import { Icon } from "../Icons";
import { providerName } from "../trackUtils";

/** Read-only status. Settings live in the server's .env file (MAGPIE_*). */
export function SettingsDrawer({ setup, onClose }: { setup: Setup; onClose: () => void }) {
  const me = useQuery({ queryKey: ["me"], queryFn: () => api<Me>("/me"), staleTime: Infinity });
  const logOut = async () => {
    await api("/logout", { method: "POST", body: "{}" }).catch(() => undefined);
    window.location.reload();
  };
  return (
    <div className="drawer-layer" role="presentation">
      <button className="drawer-backdrop" onClick={onClose} aria-label="Close status" />
      <aside
        className="settings-drawer"
        role="dialog"
        aria-modal="true"
        aria-labelledby="settings-title"
      >
        <header>
          <div>
            <p className="eyebrow">magpie-fi</p>
            <h2 id="settings-title">Status</h2>
          </div>
          <button className="icon-button" onClick={onClose} aria-label="Close status" autoFocus>
            <Icon name="x" />
          </button>
        </header>
        <div className="drawer-content">
          <section>
            <h3>Folders</h3>
            <dl className="status-list">
              <dt>Inbox</dt>
              <dd title={setup.inbox_dir}>{setup.inbox_dir || "—"}</dd>
              <dt>Library</dt>
              <dd title={setup.library_dir}>
                {setup.library_dir || "—"} · {setup.library_songs}{" "}
                {setup.library_songs === 1 ? "song" : "songs"}
              </dd>
              <dt>Trash</dt>
              <dd title={setup.trash_dir}>Originals are kept for {setup.trash_days} days</dd>
            </dl>
            <p className="settings-hint">
              New music in the inbox is checked every {setup.inbox_check_seconds} seconds. Songs
              magpie is sure about are filed automatically; the rest wait here in Review.
            </p>
          </section>
          <section>
            <h3>Navidrome</h3>
            <p className="settings-hint">
              Navidrome watches the library folder and shows new songs within seconds.
            </p>
          </section>
          <section>
            <h3>Telegram</h3>
            <p className="settings-hint">
              {setup.telegram.configured
                ? `Bot is on · ${setup.telegram.owners} owner(s), ${setup.telegram.guests} guest(s) · files up to 20 MB`
                : "Off. Set MAGPIE_TELEGRAM_BOT_TOKEN and MAGPIE_TELEGRAM_OWNERS to send links and files from Telegram."}
            </p>
          </section>
          <section>
            <h3>Downloads</h3>
            <p className="settings-hint">
              {setup.ytdlp
                ? `yt-dlp ready · ${setup.audio_format.toUpperCase()} · ${setup.max_parallel_downloads} at a time`
                : "yt-dlp is not installed, so links cannot be downloaded."}
            </p>
          </section>
          <section>
            <h3>Extras</h3>
            <p className="settings-hint">
              Lyrics: {setup.lyrics ? "on (LRCLIB)" : "off"} · Album loudness:{" "}
              {setup.rsgain ? "on (rsgain)" : "track only (rsgain not installed)"}
            </p>
          </section>
          <ProviderStatus sources={setup.sources} />
          <p className="settings-hint">
            To change settings, edit the <code>MAGPIE_…</code> values in the server&apos;s{" "}
            <code>.env</code> file and restart magpie.
          </p>
          {me.data?.via === "session" && (
            <button className="compact-button" onClick={() => void logOut()}>
              Log out of this browser
            </button>
          )}
        </div>
      </aside>
    </div>
  );
}

function ProviderStatus({ sources }: { sources: Record<string, boolean> }) {
  const available = Object.values(sources).filter(Boolean).length;
  return (
    <section className="provider-settings">
      <div className="section-heading">
        <div>
          <h3>Recognition tools</h3>
          <p>{available} sources and local tools available</p>
        </div>
        <span className="health-dot">Ready</span>
      </div>
      <div className="provider-pills">
        {[
          "musicbrainz",
          "deezer",
          "itunes",
          "radiojavan",
          "navahang",
          "audiomack",
          "genius",
          "spotify",
          "acoustid",
          "shazam",
          "songrec",
          "ffmpeg",
        ].map((source) => (
          <span className={sources[source] ? "active" : "inactive"} key={source}>
            <i />
            {providerName(source)}
          </span>
        ))}
      </div>
      {!sources.songrec && (
        <p className="tool-warning">
          <Icon name="alert" size={15} />
          Install SongRec to recognize difficult files through Shazam. Catalog matching still works
          without it.
        </p>
      )}
      {!sources.ffmpeg && (
        <p className="tool-warning">
          <Icon name="alert" size={15} />
          Install FFmpeg for audio integrity checks and ReplayGain. Metadata cleaning still works.
        </p>
      )}
    </section>
  );
}
