import { Icon, type IconName } from "../Icons";

export function EmptyState({
  icon,
  title,
  description,
  action,
  onAction,
}: {
  icon: IconName;
  title: string;
  description: string;
  action?: string;
  onAction?: () => void;
}) {
  return (
    <div className="empty-state">
      <span className="empty-icon">
        <Icon name={icon} size={30} />
      </span>
      <h2>{title}</h2>
      <p>{description}</p>
      {action && (
        <button type="button" className="text-button" onClick={onAction}>
          {action}
        </button>
      )}
    </div>
  );
}

export function ListSkeleton({ rows = 5 }: { rows?: number }) {
  return (
    <div className="list skeleton-list" aria-label="Loading">
      {Array.from({ length: rows }, (_, index) => (
        <div className="cell has-lead" key={index}>
          <span className="cell-lead">
            <i className="skeleton artwork artwork-small" />
          </span>
          <span className="cell-main">
            <i className="skeleton line" />
            <i className="skeleton line short" />
          </span>
        </div>
      ))}
    </div>
  );
}
