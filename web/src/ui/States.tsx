/**
 * Loading, empty and error states, shared by every page so they look and
 * read the same everywhere.
 */
import type { ReactNode } from "react";
import { Icon, type IconName } from "./Icon";

/** A small spinning ring. */
export function Spinner({ size = "md", label }: { size?: "sm" | "md" | "lg"; label?: string }) {
  return <span className={`spinner ${size}`} role={label ? "status" : undefined} aria-label={label} aria-hidden={label ? undefined : true} />;
}

/** Props for {@link EmptyState}. */
export interface EmptyStateProps {
  icon?: IconName;
  title: string;
  children?: ReactNode;
  /** Buttons under the text. */
  actions?: ReactNode;
  compact?: boolean;
}

/** A centred message for a list or page with nothing in it. */
export function EmptyState({ icon = "inbox", title, children, actions, compact }: EmptyStateProps) {
  return (
    <div className={compact ? "state compact" : "state"}>
      <span className="state-icon"><Icon name={icon} /></span>
      <h3 className="state-title">{title}</h3>
      {children && <div className="state-text">{children}</div>}
      {actions && <div className="state-actions">{actions}</div>}
    </div>
  );
}

/** Props for {@link ErrorState}. */
export interface ErrorStateProps {
  title?: string;
  /** The error or its message. */
  error: unknown;
  /** Shows a "Try again" button. */
  onRetry?: () => void;
  compact?: boolean;
}

/** Turns anything thrown into a readable message. */
export function errorMessage(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (typeof e === "string") return e;
  return "Something went wrong.";
}

/** A centred error with an optional retry. */
export function ErrorState({ title = "Something went wrong", error, onRetry, compact }: ErrorStateProps) {
  return (
    <div className={compact ? "state error-state compact" : "state error-state"} role="alert">
      <span className="state-icon"><Icon name="alert" /></span>
      <h3 className="state-title">{title}</h3>
      <div className="state-text">{errorMessage(error)}</div>
      {onRetry && (
        <div className="state-actions">
          <button type="button" className="btn" onClick={onRetry}>
            <Icon name="refresh" />
            Try again
          </button>
        </div>
      )}
    </div>
  );
}

/** One shimmering placeholder block. */
export function Skeleton({ width, height, radius, className }: { width?: string | number; height?: string | number; radius?: string; className?: string }) {
  return <span className={className ? `skeleton ${className}` : "skeleton"} style={{ width, height, borderRadius: radius }} aria-hidden />;
}

/** Placeholder rows for a list or table that is loading. */
export function SkeletonRows({ rows = 6 }: { rows?: number }) {
  return (
    <div className="skeleton-rows" role="status" aria-label="Loading">
      {Array.from({ length: rows }, (_, i) => (
        <div className="skeleton-row" key={i}>
          <Skeleton width={20} height={20} radius="var(--radius-sm)" />
          <Skeleton width={`${30 + ((i * 37) % 40)}%`} height={12} />
          <span className="grow" />
          <Skeleton width={64} height={12} />
          <Skeleton width={120} height={12} className="hide-phone" />
        </div>
      ))}
    </div>
  );
}

/** Placeholder tiles for a grid that is loading. */
export function SkeletonTiles({ count = 12 }: { count?: number }) {
  return (
    <div className="skeleton-tiles" role="status" aria-label="Loading">
      {Array.from({ length: count }, (_, i) => (
        <div className="skeleton-tile" key={i}>
          <Skeleton className="skeleton-thumb" />
          <Skeleton width="70%" height={10} />
        </div>
      ))}
    </div>
  );
}
