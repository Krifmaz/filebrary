//! The browser's state, shared by the listing, the toolbar, the menus and the dialogs through one `Copy`
//! handle ([`Ctx`]).

use super::api::{self, Annotation};
use super::kinds::{self, ViewMode};
use super::menu::Menu;
use super::paths::{self, SortKey};
use crate::api::{Entry, Permissions, PreviewCapabilities, Source};
use leptos::prelude::*;
use std::collections::{BTreeMap, BTreeSet};

/// The listing of the folder on screen.
#[derive(Clone, Debug, PartialEq)]
pub enum Load {
    /// Asking the server.
    Loading,
    /// The folder's entries.
    Ready(Vec<Entry>),
    /// It could not be opened (the sentence to show).
    Failed(String),
}

/// Everything the browser's parts share.
#[derive(Clone, Copy)]
pub struct Ctx {
    /// The source's id.
    pub source: Signal<String>,
    /// The folder.
    pub path: Signal<String>,
    /// The source (name and permissions), once the sources have loaded.
    pub src: Memo<Option<Source>>,
    /// The folder's listing.
    pub load: RwSignal<Load>,
    /// Stars and tags by entry name.
    pub notes: RwSignal<BTreeMap<String, Annotation>>,
    /// Selected paths.
    pub selected: RwSignal<BTreeSet<String>>,
    /// Where a Shift+click range starts.
    pub anchor: RwSignal<Option<String>>,
    /// Sort key and ascending.
    pub sort: RwSignal<(SortKey, bool)>,
    /// The folder filter.
    pub filter: RwSignal<String>,
    /// The view mode.
    pub mode: RwSignal<ViewMode>,
    /// Bumped to reload the folder.
    pub reload: RwSignal<u64>,
    /// The open context menu.
    pub menu: Menu,
    /// What the preview service can do.
    pub caps: RwSignal<Option<PreviewCapabilities>>,
    /// The file open in the viewer.
    pub viewing: RwSignal<Option<Entry>>,
    /// The signed-in account's id (the view mode is remembered per account).
    pub user: i64,
    /// Entries shown: filtered and sorted.
    pub visible: Memo<Vec<Entry>>,
}

impl Ctx {
    /// A fresh state for `source` and `path`, with `sources` for names and permissions.
    pub fn new(source: Signal<String>, path: Signal<String>, sources: LocalResource<Result<Vec<Source>, String>>, user: i64) -> Ctx {
        let src = Memo::new(move |_| {
            let id = source.get();
            sources.get().and_then(|r| r.ok()).and_then(|l| l.into_iter().find(|s| s.id == id))
        });
        let load = RwSignal::new(Load::Loading);
        let filter = RwSignal::new(String::new());
        let sort = RwSignal::new((SortKey::Name, true));
        let visible = Memo::new(move |_| match load.get() {
            Load::Ready(list) => {
                let mut v = paths::filter_entries(&list, &filter.get());
                let (k, asc) = sort.get();
                paths::sort_entries(&mut v, k, asc);
                v
            }
            _ => Vec::new(),
        });
        Ctx {
            source,
            path,
            src,
            load,
            notes: RwSignal::new(BTreeMap::new()),
            selected: RwSignal::new(BTreeSet::new()),
            anchor: RwSignal::new(None),
            sort,
            filter,
            mode: RwSignal::new(kinds::load_mode(user, &source.get_untracked(), &path.get_untracked())),
            reload: RwSignal::new(0),
            menu: RwSignal::new_local(None),
            caps: RwSignal::new(None),
            viewing: RwSignal::new(None),
            user,
            visible,
        }
    }

    /// What the account may do here (nothing until the sources have loaded).
    pub fn perms(&self) -> Permissions {
        self.src.get().map(|s| s.permissions).unwrap_or_default()
    }

    /// The source's name (its id until the sources have loaded).
    pub fn source_name(&self) -> String {
        self.src.get().map(|s| s.name).unwrap_or_else(|| self.source.get())
    }

    /// The selected entries, in the order shown.
    pub fn selection(&self) -> Vec<Entry> {
        let sel = self.selected.get();
        self.visible.get().into_iter().filter(|e| sel.contains(&e.path)).collect()
    }

    /// The selected entries without tracking (inside handlers).
    pub fn selection_now(&self) -> Vec<Entry> {
        let sel = self.selected.get_untracked();
        self.visible.get_untracked().into_iter().filter(|e| sel.contains(&e.path)).collect()
    }

    /// Selects exactly `paths`, with `anchor` for Shift+click.
    pub fn select(&self, paths: BTreeSet<String>, anchor: Option<String>) {
        self.selected.set(paths);
        self.anchor.set(anchor);
    }

    /// Selects nothing.
    pub fn clear(&self) {
        self.select(BTreeSet::new(), None);
    }

    /// Whether an entry is starred.
    pub fn starred(&self, e: &Entry) -> bool {
        self.notes.with(|n| n.get(&e.name).is_some_and(|a| a.starred))
    }

    /// Reloads the folder.
    pub fn refresh(&self) {
        self.reload.update(|n| *n += 1);
    }

    /// Changes the view mode and remembers it for this folder.
    pub fn set_mode(&self, m: ViewMode) {
        self.mode.set(m);
        kinds::save_mode(self.user, &self.source.get_untracked(), &self.path.get_untracked(), m);
    }

    /// Loads the folder (and its stars and tags) whenever the source, the folder or `reload` change. An answer for
    /// a folder that is no longer on screen is dropped.
    pub fn watch(&self) {
        let me = *self;
        let seq = StoredValue::new(0u64);
        Effect::new(move |prev: Option<(String, String)>| {
            let (source, path) = (me.source.get(), me.path.get());
            me.reload.track();
            let moved = prev.as_ref() != Some(&(source.clone(), path.clone()));
            if moved {
                me.clear();
                me.filter.set(String::new());
                me.mode.set(kinds::load_mode(me.user, &source, &path));
                me.load.set(Load::Loading);
                me.notes.set(BTreeMap::new());
            }
            let n = seq.get_value() + 1;
            seq.set_value(n);
            let (s2, p2) = (source.clone(), path.clone());
            kit_leptos::rt::spawn(async move {
                let listing = api::list(&s2, &p2).await;
                if seq.try_get_value() != Some(n) {
                    return;
                }
                match listing {
                    Ok(l) => {
                        // keep only selected paths that still exist
                        let have: BTreeSet<String> = l.entries.iter().map(|e| e.path.clone()).collect();
                        me.selected.update(|s| s.retain(|p| have.contains(p)));
                        me.load.set(Load::Ready(l.entries));
                        super::reveal::apply(&me);
                    }
                    Err(e) => {
                        // a file's own address: open its folder and show it
                        if e.status != 0 && p2 != "/" {
                            if let Ok(st) = api::stat(&s2, &p2).await {
                                if !st.is_dir {
                                    super::reveal::later(&st.name, true);
                                    crate::route::go(crate::route::Route::Files { source: s2.clone(), path: paths::parent(&p2) }, true);
                                    return;
                                }
                            }
                        }
                        if seq.try_get_value() == Some(n) {
                            me.load.set(Load::Failed(e.message));
                        }
                    }
                }
                if let Ok(a) = api::annotations(&s2, &p2).await {
                    if seq.try_get_value() == Some(n) {
                        me.notes.set(a);
                    }
                }
            });
            (source, path)
        });
        // the preview service, for thumbnails and the viewer
        kit_leptos::rt::spawn(async move {
            let c = api::preview_capabilities().await;
            let _ = me.caps.try_set(Some(c));
        });
        // a finished upload in this folder (or below it) reloads it, at most every 600 ms
        let done = super::upload::done();
        let pending = StoredValue::new(false);
        Effect::new(move |_| {
            let (n, s, p) = done.get();
            if n == 0 || s != me.source.get_untracked() {
                return;
            }
            let here = me.path.get_untracked();
            let inside = paths::parent(&p) == here || p.starts_with(&format!("{}/", here.trim_end_matches('/')));
            if !inside || pending.get_value() {
                return;
            }
            pending.set_value(true);
            kit_leptos::rt::after(600, move || {
                let _ = pending.try_set_value(false);
                if me.reload.try_update(|n| *n += 1).is_none() {}
            });
        });
    }
}
