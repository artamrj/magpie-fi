import { post } from "../../api/client";
import { useMeQuery } from "../../api/queries";
import type { Setup } from "../../api/types";
import { Icon, type IconName } from "../Icons";
import { telegram } from "../telegram";
import { providerName } from "../trackUtils";
import { Cell, Section } from "./ui";

type Fact = {
  icon: IconName;
  tone?: "good" | "warn" | "hint";
  /** Full wording, for the page list. */
  title: string;
  detail: string;
  /** A few words, for the sidebar. */
  short: string;
  /** Something the owner should fix; only these show in the sidebar. */
  problem?: boolean;
  /** One short sentence on what is wrong, for the sidebar's hover bubble. */
  tip?: string;
};

/** The server's state in plain words, shared by the page list and the sidebar. */
function facts(setup: Setup): Fact[] {
  const sourcesReady = Object.values(setup.sources).filter(Boolean).length;
  const sourcesTotal = Object.keys(setup.sources).length;
  const missing = Object.entries(setup.sources)
    .filter(([, ok]) => !ok)
    .map(([name]) => providerName(name));
  const { telegram: bot } = setup;
  const owners = `${bot.owners ?? 0} ${bot.owners === 1 ? "owner" : "owners"}`;
  const guests = `${bot.guests ?? 0} ${bot.guests === 1 ? "guest" : "guests"}`;
  const format =
    setup.audio_format === "best" ? "source codec kept" : setup.audio_format.toUpperCase();
  return [
    {
      icon: "send",
      tone: bot.configured ? "good" : "hint",
      title: bot.configured ? "Telegram bot is on" : "Telegram bot is off",
      detail: bot.configured
        ? `${owners} · ${guests} · files up to 20 MB`
        : "Set MAGPIE_TELEGRAM_BOT_TOKEN and MAGPIE_TELEGRAM_OWNERS",
      short: bot.configured ? `Bot on · ${owners}` : "Bot is off",
      problem: !bot.configured,
      tip: "Songs can't be sent from Telegram. Set the bot token and owners in .env.",
    },
    {
      icon: missing.length ? "alert" : "check",
      tone: missing.length ? "warn" : "good",
      title: `${sourcesReady} of ${sourcesTotal} sources ready`,
      detail: missing.length
        ? `Missing: ${missing.join(", ")}`
        : "Catalogs, fingerprinting and tools are all available",
      short: `${sourcesReady} of ${sourcesTotal} sources`,
      problem: missing.length > 0,
      tip: `Not available: ${missing.join(", ")}.`,
    },
    {
      icon: "folder",
      title: "Folders",
      detail: `${setup.library_dir || "library"} · inbox checked every ${setup.inbox_check_seconds} s · trash kept ${setup.trash_days} days`,
      short: `Inbox every ${setup.inbox_check_seconds} s`,
    },
    {
      icon: "music",
      title: "Downloads and extras",
      detail: [
        setup.ytdlp
          ? `${format}, ${setup.max_parallel_downloads} at a time`
          : "yt-dlp missing: links cannot be downloaded",
        setup.lyrics ? "lyrics on" : "lyrics off",
        setup.rsgain ? "album loudness" : "track loudness only",
      ].join(" · "),
      short: setup.ytdlp ? `${format} · lyrics ${setup.lyrics ? "on" : "off"}` : "No yt-dlp",
      problem: !setup.ytdlp,
      tip: "Links can't be downloaded until yt-dlp is installed.",
    },
  ];
}

const toneColor = { good: "var(--good)", warn: "var(--warn)", hint: "var(--hint)" };

async function logOut() {
  await post("/logout").catch(() => undefined);
  window.location.reload();
}

/** A browser signed in with the password can sign out; Telegram and local use cannot. */
function useCanLogOut() {
  const me = useMeQuery();
  return !telegram && me.data?.via === "session";
}

/** The full list, at the bottom of the Artists page on a phone or in Telegram. */
export function StatusSection({ setup }: { setup: Setup }) {
  const canLogOut = useCanLogOut();
  return (
    <Section title="Status" footer="Settings live in the server’s .env file (MAGPIE_…).">
      {facts(setup).map((fact) => (
        <Cell
          key={fact.title}
          lead={
            <Icon
              name={fact.icon}
              style={fact.tone ? { color: toneColor[fact.tone] } : undefined}
            />
          }
          title={fact.title}
          subtitle={fact.detail}
        />
      ))}
      {canLogOut && (
        <Cell
          tone="link"
          lead={<Icon name="logout" />}
          title="Log out of this browser"
          onClick={() => void logOut()}
        />
      )}
    </Section>
  );
}

/**
 * The sidebar foot, built from the same rows as the sidebar nav: what the server is doing,
 * then one row for each thing that needs fixing. Hover a problem for the details; the full
 * list stays on the Status section.
 */
export function SidebarStatus({ setup, activity }: { setup: Setup; activity?: string }) {
  const canLogOut = useCanLogOut();
  const problems = facts(setup).filter((fact) => fact.problem);
  return (
    <div className="side-status" aria-label="Status">
      <div className="side-status-row">
        <span className="side-status-dot">
          <i className={activity ? "busy" : ""} />
        </span>
        <span>{activity || "Ready"}</span>
        <small>{setup.library_songs.toLocaleString()} songs</small>
      </div>
      {problems.map((fact) => (
        <div
          key={fact.title}
          className="side-status-row warn"
          tabIndex={0}
          data-tip={fact.tip ?? fact.detail}
        >
          <Icon name="alert" size={20} />
          <span>{fact.short}</span>
        </div>
      ))}
      {canLogOut && (
        <button type="button" className="side-status-row" onClick={() => void logOut()}>
          <Icon name="logout" size={20} />
          <span>Log out</span>
        </button>
      )}
    </div>
  );
}
