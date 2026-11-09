import type { Setup } from "@/api/types";
import { Icon } from "../Icons";
import { providerName } from "../trackUtils";

/** Read-only status. Settings live in the server's .env file (MAGPIE_*). */
export function SettingsDrawer({ setup, onClose }: { setup: Setup; onClose: () => void }) {
  const navidrome = setup.navidrome;
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
              {!navidrome.configured
                ? "Not connected. Navidrome still finds new songs with its own scanner."
                : navidrome.reachable
                  ? `Connected to ${navidrome.url}. A scan starts after new songs are filed.`
                  : `Cannot reach ${navidrome.url}. Check the URL, user and password.`}
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
