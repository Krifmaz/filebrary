//! `/admin/branding`: the server's name, its logo and the default theme (one of the kit's themes).

use super::client::{self, Branding};
use super::{done, fail, failed, ic, loading};
use leptos::prelude::*;
use wasm_bindgen::JsCast;

/// The Branding page.
#[component]
pub fn BrandingPage() -> impl IntoView {
    let live = LocalResource::new(|| async { client::branding().await.map_err(|e| e.message) });
    let draft = RwSignal::new(None::<Branding>);
    let busy = RwSignal::new(false);
    Effect::new(move |_| {
        if let Some(Ok(b)) = live.get() {
            draft.set(Some(b));
        }
    });
    let save = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        let Some(b) = draft.get_untracked() else { return };
        busy.set(true);
        kit_leptos::rt::spawn(async move {
            match client::save_branding(&b).await {
                Ok(_) => {
                    live.refetch();
                    done("Branding saved. It shows after a reload.");
                }
                Err(e) => fail(e),
            }
            busy.set(false);
        });
    };
    let file_input = NodeRef::<leptos::html::Input>::new();
    let upload = move |e: leptos::ev::Event| {
        let Some(input) = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) else { return };
        let Some(file) = input.files().and_then(|f| f.get(0)) else { return };
        input.set_value("");
        busy.set(true);
        client::upload_logo(file, move |r| {
            busy.set(false);
            match r {
                Ok(()) => {
                    live.refetch();
                    done("Logo updated.");
                }
                Err(e) => fail(e),
            }
        });
    };
    let remove_logo = move |_| {
        kit_leptos::rt::spawn(async move {
            match client::delete_logo().await {
                Ok(()) => {
                    live.refetch();
                    done("Logo removed.");
                }
                Err(e) => fail(e),
            }
        })
    };
    let logo = move || live.get().and_then(Result::ok).and_then(|b| b.logo_url);
    let form = move || {
        draft.get().map(|b| {
            view! {
                <form class="card" id="branding-form" on:submit=save>
                    <h2>"Branding"</h2>
                    <p class="muted small">"For everyone, on the sign-in page and on share links too."</p>
                    <div class="form-grid mt">
                        <label class="field"><span>"Name"</span>
                            <input class="input" maxlength="60" placeholder="Filebrary" prop:value=b.name.clone()
                                on:input=move |e| { let v = event_target_value(&e); draft.update(|d| if let Some(d) = d { d.name = v; }); }/>
                            <small>"Shown in the sidebar, the page title and the installed app."</small>
                        </label>
                        <label class="field"><span>"Default theme"</span>
                            <select class="input" on:change=move |e| { let v = event_target_value(&e); draft.update(|d| if let Some(d) = d { d.default_theme = v; }); }>
                                {crate::theme::THEMES.iter().map(|(id, words)| { let sel = b.default_theme == *id; view! { <option value=*id selected=sel>{*words}</option> } }).collect_view()}
                            </select>
                            <small>"For accounts that have not picked one, and for share links."</small>
                        </label>
                    </div>
                    <div class="adm-actions">
                        <button class="btn primary" type="submit" disabled=move || busy.get()>"Save branding"</button>
                    </div>
                </form>
            }
        })
    };
    view! {
        {move || match live.get() {
            None => Some(view! { <div class="card">{loading()}</div> }.into_any()),
            Some(Err(e)) => Some(failed(e, move || live.refetch())),
            Some(Ok(_)) => None,
        }}
        {form}
        <section class="card" id="logo-card">
            <h2>"Logo"</h2>
            <p class="muted small">"A PNG, JPEG, WebP, GIF or SVG image under 512 KB. Square works best."</p>
            <div class="row wrap mt">
                <span class="adm-logo">{move || match logo() {
                    Some(src) => view! { <img src=src alt="Current logo"/> }.into_any(),
                    None => view! { <span class="faint small">"No logo"</span> }.into_any(),
                }}</span>
                <label class="btn">
                    <span inner_html=ic("plus")></span>"Upload logo"
                    <input type="file" class="adm-sr" node_ref=file_input accept="image/png,image/jpeg,image/webp,image/gif,image/svg+xml" on:change=upload/>
                </label>
                <Show when=move || logo().is_some()>
                    <button class="btn ghost" on:click=remove_logo>"Remove logo"</button>
                </Show>
            </div>
        </section>
    }
}
