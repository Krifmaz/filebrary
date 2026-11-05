//! Browsing: the file list, uploads, previews, the text editor, stars, tags and the trash.
//!
//! | Module | What |
//! | --- | --- |
//! | [`browser`] | `/files/<source>/<path…>`: head, toolbar, listing, phone bar, keyboard, drop to upload |
//! | [`listing`] | details table, compact list and tiles; selection, long press, drag to move |
//! | [`state`] | the shared state of one browser ([`state::Ctx`]) and loading |
//! | [`ops`] | the commands (open, download, star, rename, move, copy, delete with Undo, share, tags) and menus |
//! | [`menu`] | the context menu (a sheet on phones) |
//! | [`dialogs`] | name, folder picker, tags, comments, collections, trash — in the kit's modal |
//! | [`preview`] · [`editor`] | the viewer (pictures, video and audio with Cast, PDF, office) and the text editor |
//! | [`upload`] · [`panel`] · [`pick`] | resumable chunked uploads, their panel, files from the picker and drops |
//! | [`starred`] | `/starred` |
//! | [`api`] · [`http`] | the calls browsing makes; raw-body calls with progress and cancel |
//! | [`paths`] · [`kinds`] | pure rules: paths, names, sizes, sorting, selection; view modes, file kinds, viewers |
//!
//! Sharing lives in `pages/shares`: the browser's Share commands send the window event [`ops::SHARE_EVENT`]
//! (`filebrary:share` with `{source, paths, single_folder}`) for its dialog to answer.

mod api;
mod browser;
mod dialogs;
mod editor;
mod http;
mod icons;
mod kinds;
mod listing;
mod menu;
mod ops;
mod panel;
mod paths;
mod pick;
mod preview;
mod reveal;
mod starred;
mod state;
mod upload;

pub use api::set_star;
pub use browser::FilesPage;
/// For `pages/shares`, which answers the browser's Share commands.
#[allow(unused_imports)]
pub use ops::SHARE_EVENT;
pub use starred::StarredPage;
/// For `kit.rs`: the self-update waits while files upload (`UpdateConfig::hold`).
#[allow(unused_imports)]
pub use upload::uploading;

/// The browser's styles (tokens only, so every kit theme draws them).
const CSS: &str = include_str!("files.css");

/// Puts the browser's styles in the page's head (once): the upload panel outlives the page that started it.
pub(crate) fn ensure_css() {
    let Some(doc) = kit_leptos::rt::document() else { return };
    if doc.get_element_by_id("files-css").is_some() {
        return;
    }
    let Ok(style) = doc.create_element("style") else { return };
    let _ = style.set_attribute("id", "files-css");
    style.set_text_content(Some(CSS));
    if let Some(head) = doc.head() {
        let _ = head.append_child(&style);
    }
}
