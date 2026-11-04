/**
 * Buttons. Plain `<button>` elements are already styled by `ui.css`
 * (secondary look); this component adds variants, sizes, an icon and a
 * busy state with one consistent API.
 */
import { forwardRef, type ButtonHTMLAttributes, type ReactNode } from "react";
import { Icon, type IconName } from "./Icon";
import { Spinner } from "./States";

/** Visual weight of a button. */
export type ButtonVariant = "primary" | "secondary" | "ghost" | "danger" | "danger-solid";

/** Props for {@link Button}. */
export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: "sm" | "md" | "lg";
  /** Icon before the label. */
  icon?: IconName;
  /** Icon after the label (e.g. a dropdown chevron). */
  iconAfter?: IconName;
  /** Shows a spinner and disables the button. */
  busy?: boolean;
  /** Stretch to the container's width. */
  block?: boolean;
  children?: ReactNode;
}

/** Builds the class list for a button-like element. */
export function buttonClass(variant: ButtonVariant = "secondary", size: "sm" | "md" | "lg" = "md", extra?: string): string {
  return ["btn", variant !== "secondary" && variant, size !== "md" && size, extra].filter(Boolean).join(" ");
}

/** A button with a variant, size and optional icon. */
export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  { variant = "secondary", size = "md", icon, iconAfter, busy, block, className, children, disabled, type = "button", ...rest },
  ref,
) {
  return (
    <button
      ref={ref}
      type={type}
      className={buttonClass(variant, size, [block && "block", className].filter(Boolean).join(" ") || undefined)}
      disabled={disabled || busy}
      aria-busy={busy || undefined}
      {...rest}
    >
      {busy ? <Spinner size="sm" /> : icon && <Icon name={icon} />}
      {children}
      {iconAfter && <Icon name={iconAfter} />}
    </button>
  );
});

/** Props for {@link IconButton}. */
export interface IconButtonProps extends Omit<ButtonProps, "children" | "icon"> {
  icon: IconName;
  /** Accessible name, also shown as the tooltip. */
  label: string;
  /** Pressed state for toggles (e.g. a star). */
  pressed?: boolean;
  filled?: boolean;
}

/** A square, icon-only button with an accessible label. */
export const IconButton = forwardRef<HTMLButtonElement, IconButtonProps>(function IconButton(
  { icon, label, pressed, filled, variant = "ghost", className, ...rest },
  ref,
) {
  return (
    <Button
      ref={ref}
      variant={variant}
      className={["icon-only", className].filter(Boolean).join(" ")}
      aria-label={label}
      title={label}
      aria-pressed={pressed}
      {...rest}
    >
      <Icon name={icon} filled={filled} />
    </Button>
  );
});
