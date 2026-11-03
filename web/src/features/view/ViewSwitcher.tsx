/**
 * Segmented control for choosing the listing's view mode. Each button shows
 * its keyboard shortcut in the tooltip.
 */
import "./view.css";
import { VIEW_INFO, VIEW_MODES, type ViewMode } from "./viewMode";

/** Props for {@link ViewSwitcher}. */
export interface ViewSwitcherProps {
  mode: ViewMode;
  onChange: (mode: ViewMode) => void;
}

/** A radio group of view-mode buttons. */
export function ViewSwitcher({ mode, onChange }: ViewSwitcherProps) {
  return (
    <div className="view-switcher" role="radiogroup" aria-label="View">
      {VIEW_MODES.map((m) => (
        <button
          key={m}
          type="button"
          role="radio"
          aria-checked={m === mode}
          className={m === mode ? "active" : undefined}
          title={`${VIEW_INFO[m].label} (${VIEW_INFO[m].key})`}
          aria-label={VIEW_INFO[m].label}
          onClick={() => onChange(m)}
        >
          <span aria-hidden>{VIEW_INFO[m].icon}</span>
        </button>
      ))}
    </div>
  );
}
