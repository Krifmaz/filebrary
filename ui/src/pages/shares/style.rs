//! The sharing pages' styles, on the kit's design tokens only, so every kit theme draws them. They go into the
//! page once ([`ensure`]), the first time one of these pages or the dialog is drawn.

/// The rules.
pub const CSS: &str = r#"
.sh-head { display: flex; align-items: flex-end; justify-content: space-between; gap: 12px; flex-wrap: wrap; margin-bottom: 16px; }
.sh-head p { margin: 6px 0 0; }
.sh-tools { display: flex; gap: 10px; align-items: center; flex-wrap: wrap; margin-bottom: 12px; }
.sh-tools .input { flex: 1 1 220px; min-width: 0; }
.sh-item { align-items: flex-start; }
.sh-icon { display: inline-flex; padding: 9px; border-radius: 12px; background: var(--panel-2); color: var(--accent); flex: none; }
.sh-icon svg, .sh-badge svg { width: 18px; height: 18px; }
.sh-badge svg { width: 11px; height: 11px; }
.sh-item.dead .sh-icon { color: var(--faint); }
.sh-main { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 4px; }
.sh-title { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; font-weight: 650; min-width: 0; }
.sh-title a { color: inherit; text-decoration: none; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: 100%; }
.sh-title a:hover { text-decoration: underline; }
.sh-meta { display: flex; flex-wrap: wrap; gap: 4px 14px; font-size: 12.5px; color: var(--faint); }
.sh-actions { display: flex; gap: 6px; flex: none; align-items: center; }
.sh-url { display: flex; gap: 8px; align-items: center; }
.sh-url .input { flex: 1; min-width: 0; font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 13px; }
.sh-summary { list-style: none; padding: 0; margin: 14px 0 0; display: flex; flex-direction: column; gap: 8px; color: var(--muted); font-size: 13.5px; }
.sh-summary li { display: flex; gap: 8px; align-items: center; }
.sh-summary svg { width: 15px; height: 15px; color: var(--faint); }
.sh-form { display: flex; flex-direction: column; gap: 14px; }
.sh-pair { display: grid; grid-template-columns: 1fr 1fr; gap: 12px; }
.sh-form select.input { appearance: auto; }
@media (max-width: 640px) {
  .sh-item { flex-wrap: wrap; }
  .sh-actions { width: 100%; justify-content: flex-end; padding-left: 50px; }
  .sh-pair { grid-template-columns: 1fr; }
  .sh-hide-phone { display: none !important; }
}

/* the visitor's page */
.pub-wrap { min-height: 100dvh; display: flex; flex-direction: column; align-items: center; padding: 28px 16px 40px; gap: 18px; }
.pub-top { width: 100%; max-width: 920px; display: flex; align-items: center; justify-content: space-between; }
.pub-top .brand { display: flex; align-items: center; gap: 10px; font-weight: 700; }
.pub-top .brand img { width: 28px; height: 28px; }
.pub-card { width: 100%; max-width: 920px; }
.pub-card.narrow { max-width: 440px; }
.pub-headline { display: flex; align-items: center; gap: 12px; justify-content: space-between; flex-wrap: wrap; }
.pub-headline h1 { font-size: 20px; margin: 0; overflow-wrap: anywhere; }
.pub-headline .row { min-width: 0; }
.pub-sub { margin: 6px 0 0; color: var(--faint); font-size: 13px; display: flex; gap: 6px; align-items: center; }
.pub-sub svg { width: 14px; height: 14px; }
.pub-center { text-align: center; display: flex; flex-direction: column; align-items: center; gap: 10px; }
.pub-center h1 { font-size: 20px; margin: 4px 0 0; }
.pub-center form { width: 100%; display: flex; flex-direction: column; gap: 12px; text-align: left; margin-top: 8px; }
.pub-big { display: inline-flex; padding: 14px; border-radius: 16px; background: var(--panel-2); color: var(--accent); }
.pub-big svg { width: 28px; height: 28px; }
.pub-crumbs { display: flex; flex-wrap: wrap; align-items: center; gap: 4px; margin: 14px 0 0; font-size: 13.5px; }
.pub-crumbs a { color: var(--muted); text-decoration: none; padding: 2px 6px; border-radius: 6px; }
.pub-crumbs a:hover { background: var(--panel-2); color: var(--text); }
.pub-crumbs span.sep { color: var(--faint); }
.pub-crumbs .here { color: var(--text); font-weight: 650; padding: 2px 6px; }
.pub-tools { display: flex; gap: 10px; flex-wrap: wrap; margin: 14px 0 10px; }
.pub-tools input.input { flex: 1 1 200px; min-width: 0; }
.pub-tools select.input { flex: none; appearance: auto; }
.pub-list .list-item { padding: 9px 12px; gap: 12px; }
.pub-name { flex: 1; min-width: 0; display: flex; flex-direction: column; }
.pub-name a, .pub-name button { color: var(--text); text-decoration: none; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; background: none; border: 0; padding: 0; font: inherit; text-align: left; cursor: pointer; }
.pub-name a:hover, .pub-name button:hover { text-decoration: underline; }
.pub-name small { color: var(--faint); font-size: 12px; }
.pub-ficon { display: inline-flex; color: var(--muted); flex: none; }
.pub-ficon.dir { color: var(--accent); }
.pub-ficon svg { width: 20px; height: 20px; }
.pub-foot { color: var(--faint); font-size: 12px; }
.pub-preview { display: flex; justify-content: center; background: var(--panel-2); border-radius: 12px; overflow: hidden; }
.pub-preview img, .pub-preview video { max-width: 100%; max-height: 70vh; display: block; }
.pub-preview audio { width: 100%; margin: 24px; }
.pub-text { margin: 0; padding: 16px; width: 100%; max-height: 70vh; overflow: auto; white-space: pre-wrap; overflow-wrap: anywhere; font: 13px/1.5 ui-monospace, SFMono-Regular, Menlo, monospace; color: var(--text); }

/* the drop box */
.drop-box { border: 2px dashed var(--border); border-radius: var(--radius); padding: 30px 18px; }
.drop-box.drop-over, .drop-box:hover { border-color: var(--accent); }
.drop-list { list-style: none; padding: 0; margin: 16px 0 0; display: flex; flex-direction: column; gap: 8px; text-align: left; width: 100%; }
.drop-list li { display: flex; flex-direction: column; gap: 6px; padding: 10px 12px; border: 1px solid var(--border); border-radius: 12px; background: var(--panel); }
.drop-list .row { justify-content: space-between; }
.drop-list .name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; font-weight: 600; }
.drop-list .state { font-size: 12.5px; color: var(--faint); white-space: nowrap; }
.drop-list li.done .state { color: var(--ok); }
.drop-list li.failed .state { color: var(--err); white-space: normal; }
.drop-bar { height: 4px; border-radius: 4px; background: var(--panel-2); overflow: hidden; }
.drop-bar i { display: block; height: 100%; background: var(--grad); transition: width .2s; }
"#;

/// Puts [`CSS`] into the page's `<head>` once.
pub fn ensure() {
    const ID: &str = "fb-shares-css";
    if kit_leptos::rt::by_id(ID).is_some() {
        return;
    }
    let Some(doc) = kit_leptos::rt::document() else { return };
    let (Ok(el), Some(head)) = (doc.create_element("style"), kit_leptos::rt::qs("head")) else { return };
    el.set_id(ID);
    el.set_text_content(Some(CSS));
    let _ = head.append_child(&el);
}
