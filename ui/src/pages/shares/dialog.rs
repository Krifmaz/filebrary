//! The share dialog: a new link to files, a folder or a collection, with an optional password, expiry,
//! download limit or drop mode; then the link with Copy. The same form changes an existing link.

use super::api::{self, Change, Options, Share, Target};
use super::format::{self, EXPIRY_CHOICES};
use super::icon;
use kit_leptos::modal::{self, Modal, ModalOpts};
use leptos::prelude::*;
use std::cell::RefCell;

thread_local! {
    /// The share dialog on screen.
    static OPEN: RefCell<Option<Modal>> = const { RefCell::new(None) };
}

fn close() {
    if let Some(m) = OPEN.with(|o| o.borrow_mut().take()) {
        m.close();
    }
}

/// What the dialog does.
#[derive(Clone)]
enum Mode {
    /// Makes a link.
    Create(Target),
    /// Changes `share`, then tells the caller.
    Edit(Share, Callback<Share>),
}

/// Opens the share dialog for `paths` of `source`; `single_folder` (exactly one folder) offers a drop link.
pub fn share_dialog(source: &str, paths: Vec<String>, single_folder: bool) {
    open(Mode::Create(Target::Paths { source: source.to_string(), paths, single_folder }));
}

/// Opens the share dialog for the whole collection `id` called `name`.
#[allow(dead_code)] // opened from the Collections page, which lands in its own change
pub fn share_collection_dialog(id: i64, name: &str) {
    open(Mode::Create(Target::Collection { id, name: name.to_string() }));
}

/// Opens the dialog that changes `share`; `saved` gets the changed link.
pub(super) fn edit_dialog(share: Share, saved: Callback<Share>) {
    open(Mode::Edit(share, saved));
}

/// "Share Photos", "Share 3 items", "Share the collection Trip".
pub fn title_of(t: &Target) -> String {
    match t {
        Target::Paths { paths, .. } => match paths.as_slice() {
            [p] if format::base_name(p).is_empty() => "Share everything here".into(),
            [p] => format!("Share {}", format::base_name(p)),
            many => format!("Share {} items", many.len()),
        },
        Target::Collection { name, .. } => format!("Share the collection {name}"),
    }
}

fn open(mode: Mode) {
    close();
    let created = RwSignal::new(None::<Share>);
    let busy = RwSignal::new(false);
    let title = match &mode {
        Mode::Create(t) => title_of(t),
        Mode::Edit(s, _) => format!("Change the link to {}", format::target_words(s)),
    };
    let is_create = matches!(mode, Mode::Create(_));
    let opts = ModalOpts::with_view("", move || view! { <LinkForm mode=mode created=created busy=busy/> });
    let opts = ModalOpts {
        title: modal::Content::view(move || move || if created.get().is_some() { "Your link is ready".to_string() } else { title.clone() }),
        ..opts
    };
    let opts = opts.footer_view(move || {
        move || {
            if created.get().is_some() {
                return view! { <button class="btn primary" type="button" on:click=|_| close()>"Done"</button> }.into_any();
            }
            let label = if is_create { "Create link" } else { "Save" };
            view! {
                <button class="btn" type="button" on:click=|_| close()>"Cancel"</button>
                <button class="btn primary" type="submit" form="sh-form" disabled=move || busy.get()>
                    <span inner_html=icon(if is_create { "link" } else { "check" })></span>
                    {move || if busy.get() { if is_create { "Creating…" } else { "Saving…" } } else { label }}
                </button>
            }
            .into_any()
        }
    });
    let m = modal::open(opts);
    OPEN.with(|o| *o.borrow_mut() = Some(m));
}

/// The password part of a change.
fn password_change(had: bool, mode: &str, typed: &str) -> Result<Change<String>, &'static str> {
    match (had, mode) {
        (true, "keep") => Ok(Change::Keep),
        (true, "remove") => Ok(Change::Clear),
        (true, _) if typed.is_empty() => Err("Type the new password."),
        (_, _) if typed.is_empty() => Ok(Change::Keep),
        _ => Ok(Change::Set(typed.to_string())),
    }
}

/// The download-limit part of a change, from `was` (the link's limit now; `None` for a new link).
fn limit_change(was: Option<u64>, now: Option<u64>, editing: bool) -> Change<u64> {
    match (editing, now) {
        (true, n) if n == was => Change::Keep,
        (true, None) => Change::Clear,
        (_, Some(n)) => Change::Set(n),
        (false, None) => Change::Keep,
    }
}

#[component]
fn LinkForm(mode: Mode, created: RwSignal<Option<Share>>, busy: RwSignal<bool>) -> impl IntoView {
    let (editing, had_password, was_limit, was_drop, was_expiry) = match &mode {
        Mode::Edit(s, _) => (true, s.has_password, s.max_downloads, s.upload_only, s.expires_at.clone()),
        Mode::Create(_) => (false, false, None, false, None),
    };
    let can_drop = match &mode {
        Mode::Create(Target::Paths { single_folder, .. }) => *single_folder,
        Mode::Create(Target::Collection { .. }) => false,
        // the list doesn't say whether a one-path link is a folder; a drop link can be turned back into a plain one
        Mode::Edit(s, _) => s.upload_only,
    };
    let password = RwSignal::new(String::new());
    let pw_mode = RwSignal::new("keep".to_string());
    let expiry = RwSignal::new(if editing { "keep" } else { "never" }.to_string());
    let custom = RwSignal::new(was_expiry.as_deref().map(format::iso_to_local).unwrap_or_default());
    let limit = RwSignal::new(was_limit.map(|n| n.to_string()).unwrap_or_default());
    let drop = RwSignal::new(was_drop);
    let error = RwSignal::new(None::<String>);
    let mode = StoredValue::new(mode);

    let expiry_now = was_expiry.as_deref().map(format::date);
    let choices = EXPIRY_CHOICES
        .iter()
        .filter(|(v, _)| editing || *v != "keep")
        .map(|(v, l)| {
            let label = match (*v, &expiry_now) {
                ("keep", Some(d)) => format!("Keep: {d}"),
                ("keep", None) => "Keep: never".to_string(),
                _ => l.to_string(),
            };
            view! { <option value=*v selected=move || expiry.get() == *v>{label}</option> }
        })
        .collect_view();

    let submit = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        if busy.get_untracked() {
            return;
        }
        error.set(None);
        let fail = |m: &str| error.set(Some(m.to_string()));
        let pw = match password_change(had_password, &pw_mode.get_untracked(), &password.get_untracked()) {
            Ok(p) => p,
            Err(m) => return fail(m),
        };
        let custom_iso = format::local_to_iso(&custom.get_untracked());
        let now = format::now_ms();
        let exp = match format::expiry_of(&expiry.get_untracked(), &custom_iso, now) {
            Ok(x) => x,
            Err(m) => return fail(m),
        };
        if let Some(Some(t)) = &exp {
            if js_sys::Date::parse(t) <= now {
                return fail("Pick a time in the future.");
            }
        }
        let expires_at = match exp {
            None => Change::Keep,
            Some(None) if editing => Change::Clear,
            Some(None) => Change::Keep,
            Some(Some(t)) => Change::Set(t),
        };
        let is_drop = can_drop && drop.get_untracked();
        let max_downloads = if is_drop && !editing {
            Change::Keep
        } else {
            match format::limit_of(&limit.get_untracked()) {
                Ok(n) => limit_change(was_limit, n, editing),
                Err(m) => return fail(m),
            }
        };
        let upload_only = match (editing, can_drop) {
            (true, _) if is_drop == was_drop => None,
            (_, true) => Some(is_drop),
            _ => None,
        };
        let o = Options { password: pw, expires_at, max_downloads, upload_only };
        busy.set(true);
        let mode = mode.get_value();
        kit_leptos::rt::spawn(async move {
            let res = match &mode {
                Mode::Create(t) => api::create(t, &o).await,
                Mode::Edit(s, _) => api::update(s.id, &o).await,
            };
            busy.set(false);
            match (res, mode) {
                (Ok(s), Mode::Create(_)) => created.set(Some(s)),
                (Ok(s), Mode::Edit(_, saved)) => {
                    close();
                    kit_leptos::toasts::toast("Link saved", "ok");
                    saved.run(s);
                }
                (Err(e), _) => error.set(Some(e.message)),
            }
        });
    };

    let password_part = move || {
        if had_password {
            view! {
                <div class="field">
                    <span class="small" style="font-weight:600;color:var(--muted)">"Password"</span>
                    <div class="seg" role="radiogroup" aria-label="Password">
                        {[("keep", "Keep it"), ("change", "Change it"), ("remove", "Remove it")].map(|(v, l)| view! {
                            <button type="button" role="radio" class:on=move || pw_mode.get() == v aria-checked=move || (pw_mode.get() == v).to_string()
                                on:click=move |_| pw_mode.set(v.to_string())>{l}</button>
                        }).collect_view()}
                    </div>
                    <Show when=move || pw_mode.get() == "change">
                        <input class="input" type="password" autocomplete="new-password" placeholder="New password" aria-label="New password"
                            prop:value=move || password.get() on:input=move |e| password.set(event_target_value(&e))/>
                    </Show>
                </div>
            }
            .into_any()
        } else {
            view! {
                <label class="field"><span>"Password"</span>
                    <input class="input" type="password" name="share-password" autocomplete="new-password" placeholder="No password"
                        prop:value=move || password.get() on:input=move |e| password.set(event_target_value(&e))/>
                    <small>"Visitors type it before they see anything."</small>
                </label>
            }
            .into_any()
        }
    };

    let form = move || {
        view! {
            <form id="sh-form" class="sh-form" on:submit=submit novalidate>
                {password_part}
                <div class="sh-pair">
                    <label class="field"><span>"Expires"</span>
                        <select class="input" name="expiry" on:change=move |e| expiry.set(event_target_value(&e))>{choices.clone()}</select>
                    </label>
                    <Show when=move || !(can_drop && drop.get())>
                        <label class="field"><span>"Download limit"</span>
                            <input class="input" type="number" min="1" inputmode="numeric" name="limit" placeholder="Unlimited"
                                prop:value=move || limit.get() on:input=move |e| limit.set(event_target_value(&e))/>
                        </label>
                    </Show>
                </div>
                <Show when=move || expiry.get() == "custom">
                    <label class="field"><span>"Stops working on"</span>
                        <input class="input" type="datetime-local" name="expires-on"
                            prop:value=move || custom.get() on:input=move |e| custom.set(event_target_value(&e))/>
                    </label>
                </Show>
                <Show when=move || can_drop>
                    <div class="field">
                        <label class="switch">
                            <input type="checkbox" name="drop" prop:checked=move || drop.get() on:change=move |e| drop.set(event_target_checked(&e))/>
                            <i></i>
                            <span>"Drop link: visitors can only upload"</span>
                        </label>
                        <small>"They add files to this folder but can't see or download anything in it."</small>
                    </div>
                </Show>
                <div class="error-box" role="alert" class:hidden=move || error.get().is_none()>{move || error.get()}</div>
                <button type="submit" hidden></button>
            </form>
        }
    };

    move || match created.get() {
        Some(s) => view! { <Created share=s/> }.into_any(),
        None => form().into_any(),
    }
}

/// The new link: its address with Copy and Open, and what it allows.
#[component]
fn Created(share: Share) -> impl IntoView {
    let link = format::link(&share.token);
    let copy = {
        let link = link.clone();
        move |_| kit_leptos::clipboard::copy_then(&link, "Link copied")
    };
    let what = if share.upload_only { "Anyone with this link can add files." } else { "Anyone with this link can open it." };
    let expires =
        share.expires_at.as_deref().map(|e| format!("Stops working on {}", format::date(e))).unwrap_or_else(|| "Never expires".into());
    view! {
        <p class="muted" style="margin:0 0 12px">{what}</p>
        <div class="sh-url">
            <input class="input" readonly aria-label="Share link" id="sh-link" prop:value=link.clone() on:focus=|e| {
                use wasm_bindgen::JsCast;
                if let Some(i) = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) { i.select(); }
            }/>
            <button class="btn primary" type="button" on:click=copy><span inner_html=icon("copy")></span>"Copy"</button>
            <a class="btn icon" href=link target="_blank" rel="noopener" title="Open the link" aria-label="Open the link" inner_html=icon("external")></a>
        </div>
        <ul class="sh-summary">
            {share.has_password.then(|| view! { <li><span inner_html=icon("lock")></span>"Protected by a password"</li> })}
            <li><span inner_html=icon("clock")></span>{expires}</li>
            {share.max_downloads.map(|n| view! { <li><span inner_html=icon("download")></span>{format!("Up to {n} downloads")}</li> })}
            {share.upload_only.then(|| view! { <li><span inner_html=icon("inbox")></span>"Drop link: visitors only upload"</li> })}
        </ul>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_say_what_is_shared() {
        let t =
            |p: &[&str]| Target::Paths { source: "home".into(), paths: p.iter().map(|s| s.to_string()).collect(), single_folder: false };
        assert_eq!(title_of(&t(&["/Photos/Trip"])), "Share Trip");
        assert_eq!(title_of(&t(&["/a", "/b"])), "Share 2 items");
        assert_eq!(title_of(&Target::Collection { id: 1, name: "Best".into() }), "Share the collection Best");
    }

    #[test]
    fn passwords_change_only_when_asked() {
        assert_eq!(password_change(false, "keep", ""), Ok(Change::Keep));
        assert_eq!(password_change(false, "keep", "pw"), Ok(Change::Set("pw".into())));
        assert_eq!(password_change(true, "keep", "pw"), Ok(Change::Keep));
        assert_eq!(password_change(true, "remove", ""), Ok(Change::Clear));
        assert!(password_change(true, "change", "").is_err());
        assert_eq!(password_change(true, "change", "new"), Ok(Change::Set("new".into())));
    }

    #[test]
    fn limits_change_only_when_they_differ() {
        assert_eq!(limit_change(Some(3), Some(3), true), Change::Keep);
        assert_eq!(limit_change(Some(3), None, true), Change::Clear);
        assert_eq!(limit_change(None, Some(5), true), Change::Set(5));
        assert_eq!(limit_change(None, None, false), Change::Keep);
        assert_eq!(limit_change(None, Some(2), false), Change::Set(2));
    }
}
