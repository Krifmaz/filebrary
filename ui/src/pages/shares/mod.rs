//! Share links: the account's links and the public page a visitor sees. The scaffold draws the frame only.

use crate::pages::Placeholder;
use leptos::prelude::*;

/// `/shares`: the account's share links.
#[component]
pub fn SharesPage() -> impl IntoView {
    view! { <Placeholder title="Shared" icon="share"/> }
}

/// `/s/<token>/<path…>`: what a visitor of a share link sees, without an account.
#[component]
pub fn PublicSharePage(token: String, path: String) -> impl IntoView {
    view! {
        <div class="auth-wrap" data-token=token data-path=path>
            <div class="auth-card">
                <div class="brand">{crate::app_name()}</div>
                <p class="muted" style="text-align:center">"Shared files open here in the new UI soon."</p>
            </div>
        </div>
    }
}
