/** The centred card that the sign-in and first-run setup pages share. */
import type { FormEvent, ReactNode } from "react";
import { BrandMark } from "../theme/Brand";
import "./accounts.css";

/** Props for {@link AuthCard}. */
export interface AuthCardProps {
  title: string;
  /** Line under the title. */
  lead?: ReactNode;
  onSubmit: (e: FormEvent) => void;
  children: ReactNode;
  /** Small print under the card. */
  footer?: ReactNode;
}

/** A branded card with a form, centred on a soft accent wash. */
export function AuthCard({ title, lead, onSubmit, children, footer }: AuthCardProps) {
  return (
    <main className="auth">
      <div className="auth-inner">
        <form className="card auth-card" onSubmit={onSubmit} aria-labelledby="auth-title" noValidate={false}>
          <div className="auth-brand">
            <span className="brand"><BrandMark size={36} /></span>
          </div>
          <h1 id="auth-title" className="auth-title">{title}</h1>
          {lead && <p className="auth-lead">{lead}</p>}
          {children}
        </form>
        {footer && <p className="auth-footer">{footer}</p>}
      </div>
    </main>
  );
}
