/**
 * Segmented control for choosing the listing's view mode. Each button shows
 * its keyboard shortcut in the tooltip.
 */
import "./view.css";
import { Icon, type IconName } from "../../ui/Icon";
import { VIEW_INFO, VIEW_MODES, type ViewMode } from "./viewMode";

const ICONS: Record<ViewMode, IconName> = { details: "list", compact: "rows", small: "gridSmall", medium: "grid", large: "square" };

/** Props for {@link ViewSwitcher}. */
export interface ViewSwitcherProps {
  mode: ViewMode;
  onChange: (mode: ViewMode) => void;
}

/** A radio group of view-mode buttons. */
export function ViewSwitcher({ mode, onChange }: ViewSwitcherProps) {
  return (
    <div className="segmented view-switcher" role="radiogroup" aria-label="View">
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
          <Icon name={ICONS[m]} />
        </button>
      ))}
    </div>
  );
}
