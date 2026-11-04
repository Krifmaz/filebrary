/** Page-level building blocks: page header, card, badge and avatar. */
import type { HTMLAttributes, ReactNode } from "react";

/** Props for {@link PageHeader}. */
export interface PageHeaderProps {
  title: ReactNode;
  /** Line under the title. */
  description?: ReactNode;
  /** Buttons on the right (they wrap under the title on phones). */
  actions?: ReactNode;
  /** Something above the title, e.g. breadcrumbs. */
  eyebrow?: ReactNode;
}

/** The title row at the top of every page. */
export function PageHeader({ title, description, actions, eyebrow }: PageHeaderProps) {
  return (
    <header className="page-header">
      <div className="page-titles">
        {eyebrow}
        <h1 className="page-title">{title}</h1>
        {description && <p className="page-desc">{description}</p>}
      </div>
      {actions && <div className="page-actions">{actions}</div>}
    </header>
  );
}

/** A raised panel. `pad` adds the standard inner spacing. */
export function Card({ pad = true, className, ...rest }: HTMLAttributes<HTMLDivElement> & { pad?: boolean }) {
  return <div className={["card", pad && "pad", className].filter(Boolean).join(" ")} {...rest} />;
}

/** A small pill label. */
export function Badge({ tone = "neutral", children }: { tone?: "neutral" | "accent" | "ok" | "warn" | "danger" | "info"; children: ReactNode }) {
  return <span className={`badge ${tone}`}>{children}</span>;
}

/** A round avatar with the first letter of a name. */
export function Avatar({ name, size = 32 }: { name: string; size?: number }) {
  return (
    <span className="avatar" style={{ width: size, height: size, fontSize: size * 0.42 }} aria-hidden>
      {name.trim().charAt(0).toUpperCase() || "?"}
    </span>
  );
}
