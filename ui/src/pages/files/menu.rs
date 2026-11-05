//! The context menu: right-click, long-press, the ⋮ button and the "More" buttons open it. On a phone it is a
//! sheet at the bottom of the screen. Arrow keys move through it, Enter picks, Escape or a press outside closes.

use super::icons::icon;
use leptos::prelude::*;
use std::rc::Rc;
use wasm_bindgen::JsCast;

/// One line of the menu.
#[derive(Clone)]
pub enum MenuItem {
    /// A command.
    Action {
        /// Its words.
        label: String,
        /// Its icon.
        icon: &'static str,
        /// Its key, shown on the right ("Del").
        shortcut: &'static str,
        /// Drawn red.
        danger: bool,
        /// Greyed out.
        disabled: bool,
        /// What it does.
        run: Rc<dyn Fn()>,
    },
    /// A line between groups.
    Separator,
}

impl MenuItem {
    /// A command.
    pub fn new(label: impl Into<String>, icon: &'static str, run: impl Fn() + 'static) -> MenuItem {
        MenuItem::Action { label: label.into(), icon, shortcut: "", danger: false, disabled: false, run: Rc::new(run) }
    }

    /// With a key shown on the right.
    pub fn key(mut self, k: &'static str) -> MenuItem {
        if let MenuItem::Action { shortcut, .. } = &mut self {
            *shortcut = k;
        }
        self
    }

    /// Drawn red.
    pub fn danger(mut self) -> MenuItem {
        if let MenuItem::Action { danger, .. } = &mut self {
            *danger = true;
        }
        self
    }

    /// Greyed out when `off`.
    pub fn disabled(mut self, off: bool) -> MenuItem {
        if let MenuItem::Action { disabled, .. } = &mut self {
            *disabled = off;
        }
        self
    }
}

/// The signal that holds the open menu (local: its commands hold browser objects).
pub type Menu = RwSignal<Option<MenuState>, LocalStorage>;

/// An open menu: where, and what it holds.
#[derive(Clone)]
pub struct MenuState {
    /// Left edge, in viewport pixels.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// The lines.
    pub items: Vec<MenuItem>,
}

/// Where to put a menu of `w` × `h` asked for at (`x`, `y`) so it stays on a `vw` × `vh` screen (8 px margin).
pub fn place(x: f64, y: f64, w: f64, h: f64, vw: f64, vh: f64) -> (f64, f64) {
    let m = 8.0;
    let left = if x + w + m > vw { (x - w).max(m) } else { x };
    let top = if y + h + m > vh { (vh - h - m).max(m) } else { y };
    (left, top)
}

/// The point under an element's bottom-left corner (for a button that opens a menu); right-aligned when `end`.
pub fn below(el: &web_sys::Element, end: bool) -> (f64, f64) {
    let r = el.get_bounding_client_rect();
    (if end { r.right() - 220.0 } else { r.left() }, r.bottom() + 4.0)
}

/// The menu, drawn while `state` holds one.
#[component]
pub fn ContextMenu(state: Menu) -> impl IntoView {
    let node = NodeRef::<leptos::html::Div>::new();
    let close = move || state.set(None);
    // keep it on screen and focus the first command once drawn
    Effect::new(move |_| {
        let Some(s) = state.get() else { return };
        let Some(el) = node.get() else { return };
        let el: web_sys::HtmlElement = el.unchecked_into();
        let (vw, vh) = kit_leptos::rt::window()
            .map(|w| {
                (
                    w.inner_width().ok().and_then(|v| v.as_f64()).unwrap_or(1024.0),
                    w.inner_height().ok().and_then(|v| v.as_f64()).unwrap_or(768.0),
                )
            })
            .unwrap_or((1024.0, 768.0));
        let (w, h) = (el.offset_width() as f64, el.offset_height() as f64);
        let (l, t) = place(s.x, s.y, w, h, vw, vh);
        let _ = el.style().set_property("left", &format!("{l}px"));
        let _ = el.style().set_property("top", &format!("{t}px"));
        if let Some(b) = kit_leptos::rt::qs_in(&el, "button:not([disabled])").and_then(|b| b.dyn_into::<web_sys::HtmlElement>().ok()) {
            let _ = b.focus();
        }
    });
    let on_key = move |e: leptos::ev::KeyboardEvent| {
        let Some(el) = node.get() else { return };
        let buttons: Vec<web_sys::HtmlElement> =
            kit_leptos::rt::qsa_in(&el, "button:not([disabled])").into_iter().filter_map(|b| b.dyn_into().ok()).collect();
        let active = kit_leptos::rt::document().and_then(|d| d.active_element());
        let at = buttons.iter().position(|b| Some(b.unchecked_ref::<web_sys::Element>()) == active.as_ref());
        let step: i64 = match e.key().as_str() {
            "ArrowDown" => 1,
            "ArrowUp" => -1,
            "Home" => -1000,
            "End" => 1000,
            "Escape" | "Tab" => {
                e.prevent_default();
                close();
                return;
            }
            _ => return,
        };
        e.prevent_default();
        if buttons.is_empty() {
            return;
        }
        let n = buttons.len() as i64;
        let i = match at {
            None => 0,
            Some(_) if step == 1000 => n - 1,
            Some(_) if step == -1000 => 0,
            Some(i) => (i as i64 + step).rem_euclid(n),
        };
        let _ = buttons[i as usize].focus();
    };
    move || {
        state.get().map(|s| {
            let lines = s
                .items
                .into_iter()
                .map(|it| match it {
                    MenuItem::Separator => view! { <div class="ctx-sep" role="separator"></div> }.into_any(),
                    MenuItem::Action { label, icon: ic, shortcut, danger, disabled, run } => view! {
                        <button type="button" role="menuitem" class="ctx-item" class:danger=danger disabled=disabled
                            on:click=move |_| { state.set(None); run(); }>
                            <span class="ctx-icon" inner_html=icon(ic)></span>
                            <span class="grow ellipsis">{label}</span>
                            <span class="ctx-key">{shortcut}</span>
                        </button>
                    }
                    .into_any(),
                })
                .collect_view();
            view! {
                <div class="ctx-backdrop" on:click=move |_| close() on:contextmenu=move |e| { e.prevent_default(); close(); }></div>
                <div class="ctx-menu" role="menu" node_ref=node on:keydown=on_key style=format!("left:{}px;top:{}px", s.x, s.y)>{lines}</div>
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menus_stay_on_screen() {
        assert_eq!(place(10.0, 10.0, 200.0, 300.0, 1000.0, 800.0), (10.0, 10.0));
        assert_eq!(place(900.0, 10.0, 200.0, 300.0, 1000.0, 800.0), (700.0, 10.0));
        assert_eq!(place(10.0, 700.0, 200.0, 300.0, 1000.0, 800.0), (10.0, 492.0));
        assert_eq!(place(50.0, 10.0, 200.0, 900.0, 220.0, 800.0), (8.0, 8.0));
    }
}
