import { useEffect } from "react";
import { Icon, type IconName } from "../Icons";
import { hasTelegramButtons, useTelegramButtons, type ButtonAction } from "../telegram";

/** A grouped list with an optional uppercase header and footer, like Telegram's settings. */
export function Section({
  title,
  aside,
  footer,
  children,
  className = "",
}: {
  title?: React.ReactNode;
  aside?: React.ReactNode;
  footer?: React.ReactNode;
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <section className={`section ${className}`.trim()}>
      {title && (
        <h2>
          <span>{title}</span>
          {aside && <span className="section-aside">{aside}</span>}
        </h2>
      )}
      <div className="list">{children}</div>
      {footer && <p className="section-footer">{footer}</p>}
    </section>
  );
}

export type CellProps = {
  title: React.ReactNode;
  subtitle?: React.ReactNode;
  lead?: React.ReactNode;
  trailing?: React.ReactNode;
  chevron?: boolean;
  tone?: "default" | "link" | "accent" | "danger";
  selected?: boolean;
  disabled?: boolean;
  onClick?: () => void;
  children?: React.ReactNode;
};

/** One row of a list. With `onClick` it is a button; `children` go under the title row. */
export function Cell({
  title,
  subtitle,
  lead,
  trailing,
  chevron,
  tone = "default",
  selected,
  disabled,
  onClick,
  children,
}: CellProps) {
  const className =
    `cell tone-${tone} ${lead ? "has-lead" : ""} ${selected ? "selected" : ""}`.trim();
  const content = (
    <>
      {lead && <span className="cell-lead">{lead}</span>}
      <span className="cell-main">
        <b dir="auto">{title}</b>
        {subtitle && <small dir="auto">{subtitle}</small>}
        {children}
      </span>
      {(trailing || chevron) && (
        <span className="cell-side">
          {trailing}
          {chevron && <Icon name="chevron" size={16} className="cell-chevron" />}
        </span>
      )}
    </>
  );
  if (onClick)
    return (
      <button type="button" className={className} onClick={onClick} disabled={disabled}>
        {content}
      </button>
    );
  return <div className={className}>{content}</div>;
}

export function Tag({
  tone = "gray",
  children,
}: {
  tone?: "good" | "warn" | "bad" | "info" | "gray";
  children: React.ReactNode;
}) {
  return <span className={`tag tag-${tone}`}>{children}</span>;
}

export function Callout({
  icon = "info",
  tone = "info",
  children,
}: {
  icon?: IconName;
  tone?: "info" | "warn" | "bad" | "good";
  children: React.ReactNode;
}) {
  return (
    <div className={`callout tone-${tone}`}>
      <Icon name={icon} />
      <p>{children}</p>
    </div>
  );
}

export function Spinner({ size = 18 }: { size?: number }) {
  return <span className="spinner" style={{ width: size, height: size }} aria-label="Working" />;
}

export function PageTitle({
  title,
  subtitle,
  onBack,
  trailing,
}: {
  title: string;
  subtitle?: React.ReactNode;
  onBack?: () => void;
  trailing?: React.ReactNode;
}) {
  return (
    <header className={`page-title ${onBack ? "with-back" : ""}`.trim()}>
      {onBack && (
        <button type="button" className="back-button" onClick={onBack} aria-label="Back">
          <Icon name="back" />
        </button>
      )}
      <div>
        <h1>{title}</h1>
        {subtitle && <p>{subtitle}</p>}
      </div>
      {trailing && <div className="page-title-trailing">{trailing}</div>}
    </header>
  );
}

/**
 * The screen's bottom actions. Inside Telegram they become the native Main and Secondary buttons;
 * elsewhere they are drawn at the bottom of the page.
 */
export function BottomActions({
  primary,
  secondary,
}: {
  primary?: ButtonAction;
  secondary?: ButtonAction;
}) {
  useTelegramButtons(primary, secondary);
  useEffect(() => {
    const root = document.documentElement;
    const shown = !hasTelegramButtons && Boolean(primary || secondary);
    root.classList.toggle("has-actions", shown);
    return () => root.classList.remove("has-actions");
  }, [primary, secondary]);
  if (hasTelegramButtons || (!primary && !secondary)) return null;
  const render = (action: ButtonAction, kind: "primary" | "secondary") => (
    <button
      type="button"
      className={`action-button ${kind} ${action.destructive ? "destructive" : ""}`.trim()}
      disabled={action.disabled || action.busy}
      onClick={action.onClick}
    >
      {action.busy ? <Spinner size={18} /> : action.text}
    </button>
  );
  return (
    <div className="bottom-actions">
      {secondary && render(secondary, "secondary")}
      {primary && render(primary, "primary")}
    </div>
  );
}

/** A sheet that slides up from the bottom; closes on the backdrop or Escape. */
export function Sheet({
  title,
  subtitle,
  onClose,
  children,
}: {
  title: React.ReactNode;
  subtitle?: React.ReactNode;
  onClose: () => void;
  children: React.ReactNode;
}) {
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => event.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  return (
    <div className="sheet-layer" role="presentation">
      <button type="button" className="sheet-backdrop" onClick={onClose} aria-label="Close" />
      <div className="sheet" role="dialog" aria-modal="true">
        <span className="sheet-grip" />
        <header>
          <h3 dir="auto">{title}</h3>
          {subtitle && <p dir="auto">{subtitle}</p>}
        </header>
        {children}
      </div>
    </div>
  );
}

/** A text field styled like Telegram's, optionally with a button at its end. */
export function Field({
  icon,
  action,
  ...input
}: React.InputHTMLAttributes<HTMLInputElement> & {
  icon?: IconName;
  action?: React.ReactNode;
}) {
  return (
    <label className="field">
      {icon && <Icon name={icon} className="field-icon" />}
      <input {...input} />
      {action}
    </label>
  );
}
