/**
 * Form fields: a label, the control, an optional hint and an error, laid
 * out and announced the same way everywhere. Plain inputs, selects and
 * textareas are styled by `ui.css` too, so they can be used directly.
 */
import { cloneElement, isValidElement, useId, type InputHTMLAttributes, type ReactElement, type ReactNode } from "react";
import { Icon, type IconName } from "./Icon";

/** Props for {@link Field}. */
export interface FieldProps {
  label: ReactNode;
  /** Help text under the control. */
  hint?: ReactNode;
  /** Error under the control; also marks the control invalid. */
  error?: ReactNode;
  /** The control: an input, select or textarea element. */
  children: ReactElement<Record<string, unknown>>;
}

/**
 * Wraps one control with its label, hint and error, wiring `id`,
 * `aria-describedby` and `aria-invalid`.
 */
export function Field({ label, hint, error, children }: FieldProps) {
  const auto = useId();
  const id = (children.props.id as string | undefined) ?? auto;
  const hintId = hint ? `${id}-hint` : undefined;
  const errId = error ? `${id}-error` : undefined;
  const control = isValidElement(children)
    ? cloneElement(children, {
        id,
        "aria-invalid": error ? true : undefined,
        "aria-describedby": [hintId, errId].filter(Boolean).join(" ") || undefined,
      })
    : children;
  return (
    <div className={error ? "field invalid" : "field"}>
      <label className="field-label" htmlFor={id}>{label}</label>
      {control}
      {hint && <p className="field-hint" id={hintId}>{hint}</p>}
      {error && <p className="field-error" id={errId} role="alert">{error}</p>}
    </div>
  );
}

/** Props for {@link SearchInput}. */
export interface SearchInputProps extends Omit<InputHTMLAttributes<HTMLInputElement>, "type"> {
  icon?: IconName;
  /** Shows a clear button while there is text. */
  onClear?: () => void;
}

/** A text input with a leading icon and an optional clear button. */
export function SearchInput({ icon = "search", onClear, className, value, ...rest }: SearchInputProps) {
  return (
    <span className={className ? `input-icon ${className}` : "input-icon"}>
      <Icon name={icon} />
      <input type="search" value={value} {...rest} />
      {onClear && value ? (
        <button type="button" className="input-clear" aria-label="Clear" onClick={onClear}>
          <Icon name="x" />
        </button>
      ) : null}
    </span>
  );
}

/** Props for {@link Switch}. */
export interface SwitchProps extends Omit<InputHTMLAttributes<HTMLInputElement>, "type"> {
  label: ReactNode;
}

/** An on/off toggle built on a checkbox, so it works with forms and keyboards. */
export function Switch({ label, className, ...rest }: SwitchProps) {
  return (
    <label className={className ? `switch ${className}` : "switch"}>
      <input type="checkbox" role="switch" {...rest} />
      <span className="switch-track" aria-hidden />
      <span>{label}</span>
    </label>
  );
}
