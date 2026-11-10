import type { QueueFilter } from "../trackUtils";

/** The workspace views: Get music, all tracks, and the status filters. */
export function ViewTabs({
  className,
  view,
  filter,
  counts,
  total,
  onGet,
  onFilter,
}: {
  className: string;
  view: "get" | "queue";
  filter: QueueFilter;
  counts: { review: number; problems: number; ready: number };
  total: number;
  onGet: () => void;
  onFilter: (filter: QueueFilter) => void;
}) {
  const tab = (filterValue: QueueFilter, label: string, count: number, tabClass = "") => (
    <Tab
      className={tabClass}
      active={view === "queue" && filter === filterValue}
      onClick={() => onFilter(filterValue)}
      label={label}
      count={count}
    />
  );
  return (
    <nav className={className} aria-label="Workspace views">
      <Tab active={view === "get"} onClick={onGet} label="Get music" />
      {tab("all", "All tracks", total, "all-tracks-tab")}
      <div className="status-tabs" role="group" aria-label="Filter tracks by status">
        {tab("review", "Review", counts.review)}
        {tab("problems", "Issues", counts.problems)}
        {tab("ready", "Ready", counts.ready)}
      </div>
    </nav>
  );
}

function Tab({
  active,
  label,
  count,
  className = "",
  onClick,
}: {
  active: boolean;
  label: string;
  count?: number;
  className?: string;
  onClick: () => void;
}) {
  return (
    <button
      className={`${className}${active ? " active" : ""}`.trim() || undefined}
      aria-current={active ? "page" : undefined}
      onClick={onClick}
    >
      {label}
      {Boolean(count) && <span>{count}</span>}
    </button>
  );
}
