/**
 * Menus: a floating list of commands used for dropdowns ({@link MenuButton})
 * and right-click menus ({@link Menu} at a point).
 *
 * Keyboard: arrows move, Home/End jump, Enter/Space run, Escape and Tab
 * close and return focus. Clicking outside, scrolling or resizing closes it.
 * The menu is clamped to the viewport.
 */
import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { Icon, type IconName } from "./Icon";

/** One command, or a separator. */
export type MenuEntry =
  | {
      label: string;
      icon?: IconName;
      onSelect: () => void;
      disabled?: boolean;
      danger?: boolean;
      /** Shortcut hint shown on the right, e.g. "F2". */
      shortcut?: string;
      /** Checked state for toggles and choices. */
      checked?: boolean;
    }
  | { separator: true }
  | { heading: string };

/** Where to put the menu: a point (right-click) or below an element. */
export type MenuAnchor = { x: number; y: number } | { element: HTMLElement; align?: "start" | "end" };

/** Props for {@link Menu}. */
export interface MenuProps {
  anchor: MenuAnchor;
  items: MenuEntry[];
  onClose: () => void;
  label?: string;
}

const isItem = (e: MenuEntry): e is Extract<MenuEntry, { label: string }> => "label" in e;

/** A floating menu rendered in a portal. */
export function Menu({ anchor, items, onClose, label = "Menu" }: MenuProps) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  const returnFocus = useRef<Element | null>(typeof document !== "undefined" ? document.activeElement : null);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const w = el.offsetWidth;
    const h = el.offsetHeight;
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    let left: number;
    let top: number;
    if ("element" in anchor) {
      const r = anchor.element.getBoundingClientRect();
      left = anchor.align === "end" ? r.right - w : r.left;
      top = r.bottom + 6;
      if (top + h > vh - 8) top = Math.max(8, r.top - h - 6);
    } else {
      left = anchor.x;
      top = anchor.y;
      if (top + h > vh - 8) top = Math.max(8, top - h);
    }
    left = Math.min(Math.max(8, left), vw - w - 8);
    top = Math.min(Math.max(8, top), vh - h - 8);
    setPos({ left, top });
  }, [anchor]);

  const closeRef = useRef(onClose);
  useLayoutEffect(() => {
    closeRef.current = onClose;
  });
  const anchorEl = "element" in anchor ? anchor.element : null;

  useEffect(() => {
    const onClose = () => closeRef.current();
    const first = ref.current?.querySelector<HTMLElement>('[role^="menuitem"]:not([aria-disabled="true"])');
    first?.focus();
    const away = (e: Event) => {
      if (e.target instanceof Node && (ref.current?.contains(e.target) || anchorEl?.contains(e.target))) return;
      onClose();
    };
    const close = () => onClose();
    document.addEventListener("pointerdown", away, true);
    document.addEventListener("contextmenu", away, true);
    window.addEventListener("resize", close);
    window.addEventListener("scroll", close, true);
    const restore = returnFocus.current;
    return () => {
      document.removeEventListener("pointerdown", away, true);
      document.removeEventListener("contextmenu", away, true);
      window.removeEventListener("resize", close);
      window.removeEventListener("scroll", close, true);
      if (restore instanceof HTMLElement && document.contains(restore)) restore.focus();
    };
  }, [anchorEl]);

  const onKeyDown = (e: React.KeyboardEvent) => {
    const all = [...(ref.current?.querySelectorAll<HTMLElement>('[role^="menuitem"]:not([aria-disabled="true"])') ?? [])];
    const i = all.indexOf(document.activeElement as HTMLElement);
    const go = (n: number) => all[(n + all.length) % all.length]?.focus();
    if (e.key === "ArrowDown") go(i + 1);
    else if (e.key === "ArrowUp") go(i < 0 ? all.length - 1 : i - 1);
    else if (e.key === "Home") go(0);
    else if (e.key === "End") go(all.length - 1);
    else if (e.key === "Escape" || e.key === "Tab") onClose();
    else return;
    e.preventDefault();
    e.stopPropagation();
  };

  return createPortal(
    <div
      ref={ref}
      className="menu"
      role="menu"
      aria-label={label}
      style={pos ? { left: pos.left, top: pos.top } : { left: -9999, top: -9999 }}
      onKeyDown={onKeyDown}
      onContextMenu={(e) => e.preventDefault()}
    >
      {items.map((it, i) =>
        "separator" in it ? (
          <div key={i} className="menu-sep" role="separator" />
        ) : "heading" in it ? (
          <div key={i} className="menu-heading">{it.heading}</div>
        ) : (
          <button
            key={i}
            type="button"
            role={it.checked === undefined ? "menuitem" : "menuitemradio"}
            aria-checked={it.checked}
            aria-disabled={it.disabled || undefined}
            className={it.danger ? "menu-item danger" : "menu-item"}
            tabIndex={-1}
            onClick={() => {
              if (it.disabled) return;
              onClose();
              it.onSelect();
            }}
          >
            <span className="menu-icon">{it.checked ? <Icon name="check" /> : it.icon && <Icon name={it.icon} />}</span>
            <span className="menu-label">{it.label}</span>
            {it.shortcut && <kbd className="menu-kbd">{it.shortcut}</kbd>}
          </button>
        ),
      )}
    </div>,
    document.body,
  );
}

/** Props for {@link MenuButton}. */
export interface MenuButtonProps {
  items: MenuEntry[];
  /** Renders the trigger; spread `props` onto a button. */
  children: (props: {
    ref: (el: HTMLButtonElement | null) => void;
    onClick: () => void;
    "aria-haspopup": "menu";
    "aria-expanded": boolean;
  }) => ReactNode;
  align?: "start" | "end";
  label?: string;
}

/** A trigger that opens a dropdown menu below itself. */
export function MenuButton({ items, children, align = "start", label }: MenuButtonProps) {
  const [el, setEl] = useState<HTMLButtonElement | null>(null);
  const [open, setOpen] = useState(false);
  return (
    <>
      {children({ ref: setEl, onClick: () => setOpen((o) => !o), "aria-haspopup": "menu", "aria-expanded": open })}
      {open && el && <Menu anchor={{ element: el, align }} items={items} onClose={() => setOpen(false)} label={label} />}
    </>
  );
}

/** Whether a menu has at least one enabled command. */
export const hasEnabled = (items: MenuEntry[]) => items.some((i) => isItem(i) && !i.disabled);
