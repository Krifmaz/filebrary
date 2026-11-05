//! `/admin/users`: a searchable list of accounts, the New user dialog (with an optional home folder) and one
//! dialog per account: role and disabling, password reset, home folder and permissions per source, groups,
//! storage quota, two-factor reset and deletion.

use super::client::{self, Account, Rule, Subject, PERMS, ROLES};
use super::settings::{initial, role_badge, QuotaBar};
use super::{done, empty, fail, failed, ic, loading};
use crate::api::{Role, Source};
use kit_leptos::modal::{self, ModalOpts};
use kit_rules::dialogs::Confirm;
use leptos::prelude::*;

type Sources = LocalResource<Result<Vec<Source>, String>>;

/// What an account reaches, in a few words: "Every source", the sources it has rules on, or none.
pub fn access_summary(a: &Account, rules: &[Rule], name_of: impl Fn(&str) -> String) -> String {
    if a.role == Role::Admin {
        return "Every source".into();
    }
    let mut names: Vec<String> = Vec::new();
    for r in rules.iter().filter(|r| r.subject == Subject::User(a.id) && !r.perms.is_empty()) {
        let n = name_of(&r.source);
        if !names.contains(&n) {
            names.push(n);
        }
    }
    if names.is_empty() {
        "No access of their own (groups may give some)".into()
    } else {
        names.join(", ")
    }
}

/// Whether an account matches the search words (its name or its role).
pub fn matches(a: &Account, q: &str) -> bool {
    let q = q.trim().to_lowercase();
    q.is_empty() || a.username.to_lowercase().contains(&q) || a.role.label().to_lowercase().contains(&q)
}

/// The role picker with the line saying what the role allows.
#[component]
fn RoleSelect(role: RwSignal<Role>, #[prop(optional)] disabled: bool) -> impl IntoView {
    view! {
        <label class="field"><span>"Role"</span>
            <select class="input" disabled=disabled on:change=move |e| role.set(client::role_of(&event_target_value(&e)))>
                {ROLES.iter().map(|(r, id)| {
                    let r = *r;
                    view! { <option value=*id selected=move || role.get() == r>{r.label()}</option> }
                }).collect_view()}
            </select>
            <small>{move || client::role_hint(role.get())}</small>
        </label>
    }
}

/// A password box with Generate.
#[component]
fn PasswordBox(pw: RwSignal<String>, #[prop(optional)] children: Option<Children>) -> impl IntoView {
    view! {
        <span class="adm-pw">
            <input class="input mono" type="text" autocomplete="new-password" spellcheck="false"
                prop:value=move || pw.get() on:input=move |e| pw.set(event_target_value(&e))/>
            <button class="btn" type="button" on:click=move |_| pw.set(client::generate_password())>
                <span inner_html=ic("refresh")></span>"Generate"
            </button>
            {children.map(|c| c())}
        </span>
    }
}

/// The Users page.
#[component]
pub fn UsersPage(me: i64, sources: Sources) -> impl IntoView {
    let users = LocalResource::new(|| async { client::users().await.map_err(|e| e.message) });
    let rules = LocalResource::new(|| async { client::rules().await.unwrap_or_default() });
    let q = RwSignal::new(String::new());
    let reload = move || {
        users.refetch();
        rules.refetch();
    };
    let source_list = move || sources.get().and_then(Result::ok).unwrap_or_default();
    let open_edit = move |a: Account| edit_dialog(a, me, source_list(), Callback::new(move |_| reload()));
    let open_new = move |_| {
        new_dialog(
            source_list(),
            Callback::new(move |a: Account| {
                reload();
                open_edit(a);
            }),
        )
    };

    let list = move || {
        let all = match users.get() {
            None => return view! { <div class="card">{loading()}</div> }.into_any(),
            Some(Err(e)) => return failed(e, move || users.refetch()),
            Some(Ok(all)) => all,
        };
        let shown: Vec<Account> = all.iter().filter(|a| matches(a, &q.get())).cloned().collect();
        if shown.is_empty() {
            let (t, l) = if q.get().trim().is_empty() {
                ("No users yet", "Create the first account with New user.")
            } else {
                ("No matching users", "Try another name.")
            };
            return view! { <div class="card">{empty("users", t, l)}</div> }.into_any();
        }
        let rules_now = rules.get().unwrap_or_default();
        let srcs = source_list();
        let name_of = move |id: &str| srcs.iter().find(|s| s.id == id).map(|s| s.name.clone()).unwrap_or_else(|| id.to_string());
        view! {
            <ul class="card flat list adm-users" aria-label="Users">
                {shown.into_iter().map(|a| {
                    let summary = access_summary(&a, &rules_now, &name_of);
                    let a2 = a.clone();
                    view! {
                        <li>
                            <button type="button" class="adm-user" aria-label=format!("Edit {}", a.username) on:click=move |_| open_edit(a2.clone())>
                                <span class="adm-avatar">{initial(&a.username)}</span>
                                <span class="grow adm-user-main">
                                    <span class="row wrap adm-user-title">
                                        <strong class="ellipsis">{a.username.clone()}</strong>
                                        {(a.id == me).then(|| view! { <span class="badge ok">"You"</span> })}
                                        {a.disabled.then(|| view! { <span class="badge err">"Disabled"</span> })}
                                        {(a.auth_source != "local").then(|| view! { <span class="badge info">{a.auth_source.to_uppercase()}</span> })}
                                    </span>
                                    <span class="faint small ellipsis">{summary}</span>
                                </span>
                                {role_badge(a.role)}
                                <span class="faint small adm-hide-phone">{format!("Since {}", client::date(Some(a.created_at)))}</span>
                            </button>
                        </li>
                    }
                }).collect_view()}
            </ul>
        }
        .into_any()
    };
    let count = move || match users.get() {
        Some(Ok(all)) => format!("{} {}", all.len(), if all.len() == 1 { "account" } else { "accounts" }),
        _ => String::new(),
    };
    view! {
        <div class="adm-toolbar">
            <input class="input adm-find" type="search" placeholder="Find a user" aria-label="Find a user"
                prop:value=move || q.get() on:input=move |e| q.set(event_target_value(&e))/>
            <span class="faint small grow">{count}</span>
            <button class="btn primary" id="new-user" on:click=open_new><span inner_html=ic("plus")></span>"New user"</button>
        </div>
        {list}
    }
}

/// The New user dialog; `created` gets the new account.
fn new_dialog(sources: Vec<Source>, created: Callback<Account>) {
    let body = move || {
        let username = RwSignal::new(String::new());
        let password = RwSignal::new(String::new());
        let role = RwSignal::new(Role::User);
        let home = RwSignal::new(!sources.is_empty());
        let home_source = RwSignal::new(sources.first().map(|s| s.id.clone()).unwrap_or_default());
        let home_path = RwSignal::new(String::new());
        let error = RwSignal::new(None::<String>);
        let busy = RwSignal::new(false);
        let has_sources = !sources.is_empty();
        let sources = StoredValue::new(sources);
        let submit = move |e: leptos::ev::SubmitEvent| {
            e.prevent_default();
            if let Some(p) = crate::api::password_problem(&password.get_untracked(), None) {
                error.set(Some(p.into()));
                return;
            }
            busy.set(true);
            error.set(None);
            kit_leptos::rt::spawn(async move {
                let src = home_source.get_untracked();
                let path = home_path.get_untracked();
                let with_home = home.get_untracked() && role.get_untracked() != Role::Admin && !src.is_empty();
                match client::create_user(
                    username.get_untracked().trim(),
                    &password.get_untracked(),
                    role.get_untracked(),
                    with_home.then_some((src.as_str(), path.as_str())),
                )
                .await
                {
                    Ok(a) => {
                        modal::close_all();
                        done(&format!("Created {}.", a.username));
                        created.run(a);
                    }
                    Err(e) => {
                        error.set(Some(e.message));
                        busy.set(false);
                    }
                }
            });
        };
        let placeholder =
            move || format!("/home/{}", if username.get().trim().is_empty() { "name".into() } else { username.get().trim().to_string() });
        view! {
            <form class="stack" id="new-user-form" on:submit=submit>
                <p class="muted small">"They can change the password after signing in."</p>
                <label class="field"><span>"Username"</span>
                    <input class="input" autocomplete="off" required maxlength="64" autofocus
                        prop:value=move || username.get() on:input=move |e| username.set(event_target_value(&e))/>
                    <small>"Letters, digits, '.', '_', '-' or '@'."</small>
                </label>
                <label class="field"><span>"Password"</span><PasswordBox pw=password/></label>
                <RoleSelect role=role/>
                <Show when=move || has_sources && role.get() != Role::Admin>
                    <label class="switch">
                        <input type="checkbox" prop:checked=move || home.get() on:change=move |e| home.set(event_target_checked(&e))/>
                        <i></i><span>"Give them a home folder"</span>
                    </label>
                    <Show when=move || home.get()>
                        <div class="form-grid">
                            <label class="field"><span>"On source"</span>
                                <select class="input" on:change=move |e| home_source.set(event_target_value(&e))>
                                    {sources.get_value().into_iter().map(|s| {
                                        let id = s.id.clone();
                                        let id2 = id.clone();
                                        view! { <option value=id selected=move || home_source.get() == id2>{s.name.clone()}</option> }
                                    }).collect_view()}
                                </select>
                            </label>
                            <label class="field"><span>"Folder"</span>
                                <input class="input" placeholder=placeholder prop:value=move || home_path.get() on:input=move |e| home_path.set(event_target_value(&e))/>
                                <small>"Made if missing. They see it as the top of the source."</small>
                            </label>
                        </div>
                    </Show>
                </Show>
                <p class="field-err small" role="alert" class:hidden=move || error.get().is_none()>{move || error.get()}</p>
                <div class="adm-actions">
                    <button class="btn ghost" type="button" data-close="">"Cancel"</button>
                    <button class="btn primary" type="submit" disabled=move || busy.get() || username.get().trim().is_empty() || password.get().is_empty()>"Create user"</button>
                </div>
            </form>
        }
    };
    modal::open(ModalOpts::with_view("New user", body));
}

/// The dialog of one account; `changed` runs after every change so the list follows.
fn edit_dialog(account: Account, me: i64, sources: Vec<Source>, changed: Callback<()>) {
    let title = account.username.clone();
    let body = move || {
        let self_ = account.id == me;
        let current = RwSignal::new(account.clone());
        let role = RwSignal::new(account.role);
        let disabled = RwSignal::new(account.disabled);
        let busy = RwSignal::new(false);
        let save = move |_| {
            busy.set(true);
            kit_leptos::rt::spawn(async move {
                match client::update_user(current.get_untracked().id, role.get_untracked(), disabled.get_untracked()).await {
                    Ok(a) => {
                        current.set(a);
                        done("Saved.");
                        changed.run(());
                    }
                    Err(e) => fail(e),
                }
                busy.set(false);
            });
        };
        let dirty = move || role.get() != current.get().role || disabled.get() != current.get().disabled;
        let a = account.clone();
        let how = if a.auth_source == "local" {
            "password sign-in".to_string()
        } else {
            format!("signs in with {}", a.auth_source.to_uppercase())
        };
        let remove = {
            let a = a.clone();
            move |_| {
                let a = a.clone();
                kit_leptos::rt::spawn(async move {
                    let q = Confirm::new(format!("Delete {}?", a.username))
                        .text("Their sessions, keys, rules and share links go too. Their files stay where they are.")
                        .ok("Delete user")
                        .danger(true);
                    if !modal::confirm(q).await {
                        return;
                    }
                    match client::delete_user(a.id).await {
                        Ok(()) => {
                            modal::close_all();
                            done(&format!("Deleted {}.", a.username));
                            changed.run(());
                        }
                        Err(e) => fail(e),
                    }
                });
            }
        };
        let reset_2fa = {
            let a = a.clone();
            move |_| {
                let a = a.clone();
                kit_leptos::rt::spawn(async move {
                    let q = Confirm::new(format!("Turn off two-factor sign-in for {}?", a.username))
                        .text("Use this when they lost their phone. They can set it up again in Settings.")
                        .ok("Reset two-factor")
                        .danger(true);
                    if !modal::confirm(q).await {
                        return;
                    }
                    match client::reset_totp(a.id).await {
                        Ok(()) => done("Two-factor sign-in reset."),
                        Err(e) => fail(e),
                    }
                });
            }
        };
        let not_admin = move || current.get().role != Role::Admin;
        let read_only = move || current.get().role == Role::ReadOnly;
        let local = a.auth_source == "local";
        let sources2 = sources.clone();
        view! {
            <div class="stack adm-edit" id="edit-user">
                <p class="muted small">{format!("{} · {}", a.role.label(), how)}</p>
                <section class="adm-section" aria-labelledby="sec-account">
                    <h3 id="sec-account">"Account"</h3>
                    <RoleSelect role=role disabled=self_/>
                    <label class="switch">
                        <input type="checkbox" disabled=self_ prop:checked=move || disabled.get() on:change=move |e| disabled.set(event_target_checked(&e))/>
                        <i></i><span>"Account disabled"</span>
                    </label>
                    <p class="faint small">{if self_ { "You can't change your own role or disable yourself." } else { "A disabled account can't sign in, and its sessions end." }}</p>
                    <div class="adm-actions"><button class="btn primary" disabled=move || !dirty() || busy.get() on:click=save>"Save"</button></div>
                </section>
                {local.then(|| view! { <ResetPassword id=a.id name=a.username.clone()/> })}
                <Show when=not_admin>
                    <section class="adm-section" aria-labelledby="sec-access">
                        <h3 id="sec-access">"Access"</h3>
                        <p class="muted small">"A home folder keeps them to one folder of a source, which they see as its top. Permissions cover that folder and everything inside."
                            {move || read_only().then_some(" As a read-only account they can only view and download, whatever is ticked.")}</p>
                        <SourcesAccess id=account.id name=account.username.clone() sources=sources2.clone() read_only=Signal::derive(read_only) changed=changed/>
                    </section>
                </Show>
                <Groups id=a.id/>
                <QuotaSection id=a.id/>
                <section class="adm-section adm-danger" aria-labelledby="sec-danger">
                    <h3 id="sec-danger">"Danger zone"</h3>
                    <div class="row wrap">
                        <button class="btn" on:click=reset_2fa>"Reset two-factor"</button>
                        <button class="btn danger" disabled=self_ on:click=remove><span inner_html=ic("trash")></span>"Delete user"</button>
                    </div>
                </section>
            </div>
        }
    };
    modal::open(ModalOpts::with_view(&title, body).wide(true).footer_view(|| view! { <button class="btn" data-close="">"Done"</button> }));
}

/// Reset password: a new one, given to them privately.
#[component]
fn ResetPassword(id: i64, name: String) -> impl IntoView {
    let pw = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let reset = move |_| {
        if let Some(p) = crate::api::password_problem(&pw.get_untracked(), None) {
            error.set(Some(p.into()));
            return;
        }
        busy.set(true);
        error.set(None);
        let name = name.clone();
        kit_leptos::rt::spawn(async move {
            match client::reset_password(id, &pw.get_untracked()).await {
                Ok(()) => {
                    pw.set(String::new());
                    done(&format!("Password reset. {name} was signed out everywhere."));
                }
                Err(e) => error.set(Some(e.message)),
            }
            busy.set(false);
        });
    };
    view! {
        <section class="adm-section" aria-labelledby="sec-pw">
            <h3 id="sec-pw">"Reset password"</h3>
            <label class="field"><span>"New password"</span>
                <PasswordBox pw=pw>
                    <button class="btn primary" type="button" disabled=move || busy.get() || pw.get().is_empty() on:click=reset.clone()>"Reset"</button>
                </PasswordBox>
                <small>"Give it to them privately; they can change it in Settings."</small>
            </label>
            <p class="field-err small" role="alert" class:hidden=move || error.get().is_none()>{move || error.get()}</p>
        </section>
    }
}

/// The access block of every source, from one load of the rules and the home folders.
#[component]
fn SourcesAccess(id: i64, name: String, sources: Vec<Source>, read_only: Signal<bool>, changed: Callback<()>) -> impl IntoView {
    let rules = LocalResource::new(|| async { client::rules().await.map_err(|e| e.message) });
    let scopes = LocalResource::new(move || async move { client::scopes(id).await.map_err(|e| e.message) });
    let reload = Callback::new(move |_| {
        rules.refetch();
        scopes.refetch();
        changed.run(());
    });
    if sources.is_empty() {
        return view! { <p class="faint small">"There are no sources."</p> }.into_any();
    }
    (move || match (rules.get(), scopes.get()) {
        (Some(Err(e)), _) | (_, Some(Err(e))) => failed(e, move || {
            rules.refetch();
            scopes.refetch();
        }),
        (Some(Ok(rs)), Some(Ok(sc))) => sources
            .iter()
            .map(|s| {
                let scope = sc.get(&s.id).cloned().unwrap_or_else(|| "/".into());
                let own: Vec<Rule> = rs.iter().filter(|r| r.source == s.id && r.subject == Subject::User(id)).cloned().collect();
                view! { <SourceAccess id=id name=name.clone() source=s.clone() scope=scope own=own read_only=read_only reload=reload/> }
            })
            .collect_view()
            .into_any(),
        _ => loading(),
    })
    .into_any()
}

/// One source: the home folder, the permissions there, and the account's other rules on it.
#[component]
fn SourceAccess(
    id: i64,
    name: String,
    source: Source,
    scope: String,
    own: Vec<Rule>,
    read_only: Signal<bool>,
    reload: Callback<()>,
) -> impl IntoView {
    let at_scope = own.iter().find(|r| r.path == scope).map(|r| r.perms.clone()).unwrap_or_default();
    let perms = RwSignal::new(at_scope.clone());
    let perms_dirty = RwSignal::new(false);
    let home = RwSignal::new(if scope == "/" { String::new() } else { scope.clone() });
    let start_home = home.get_untracked();
    let busy = RwSignal::new(false);
    let sid = source.id.clone();
    let sname = source.name.clone();
    let scope2 = scope.clone();
    let save = move |_| {
        busy.set(true);
        let (sid, sname, scope) = (sid.clone(), sname.clone(), scope2.clone());
        kit_leptos::rt::spawn(async move {
            let result: crate::api::Result<()> = async {
                let target = Some(home.get_untracked().trim().to_string()).filter(|t| !t.is_empty()).unwrap_or_else(|| "/".into());
                let mut path = scope.clone();
                if target != scope {
                    if target == "/" {
                        client::set_scope(id, &sid, "/").await?;
                        path = "/".into();
                    } else {
                        // makes the folder and grants every permission there
                        path = client::set_home(id, &sid, &target).await?;
                    }
                }
                // a new home folder keeps its full grant unless the boxes were changed
                if perms_dirty.get_untracked() || target == scope {
                    let p = perms.get_untracked();
                    if !p.is_empty() {
                        client::set_rule(&sid, &path, Subject::User(id), &p).await?;
                    } else if let Some(r) =
                        client::rules().await?.into_iter().find(|r| r.source == sid && r.subject == Subject::User(id) && r.path == path)
                    {
                        client::delete_rule(r.id).await?;
                    }
                }
                Ok(())
            }
            .await;
            match result {
                Ok(()) => {
                    done(&format!("Saved access to {sname}."));
                    reload.run(());
                }
                Err(e) => fail(e),
            }
            busy.set(false);
        });
    };
    let dirty = move || perms_dirty.get() || home.get() != start_home;
    let others: Vec<Rule> = own.into_iter().filter(|r| r.path != scope).collect();
    let placeholder = format!("/home/{name}");
    let badge = move || {
        let p = perms.get();
        let (cls, words) = match p.len() {
            0 => ("badge", "No access"),
            n if n == PERMS.len() => ("badge ok", "Full access"),
            _ => ("badge ok", "Some access"),
        };
        view! { <span class=cls>{words}</span> }
    };
    view! {
        <div class="adm-source" data-source=source.id.clone()>
            <div class="row between"><strong>{source.name.clone()}</strong>{badge}</div>
            <label class="field"><span>"Home folder"</span>
                <input class="input" placeholder=placeholder prop:value=move || home.get() on:input=move |e| home.set(event_target_value(&e))/>
                <small>"Empty: the whole source."</small>
            </label>
            <div class="adm-perms" role="group" aria-label=format!("Permissions on {}", source.name)>
                {PERMS.iter().map(|p| {
                    let p = *p;
                    let capped = move || read_only.get() && p != "view" && p != "download";
                    view! {
                        <label class="adm-perm" class:on=move || perms.get().iter().any(|x| x == p) class:capped=capped>
                            <input type="checkbox" prop:checked=move || perms.get().iter().any(|x| x == p)
                                on:change=move |e| { let on = event_target_checked(&e); perms.update(|ps| *ps = client::toggle_perm(ps, p, on)); perms_dirty.set(true); }/>
                            {p}
                        </label>
                    }
                }).collect_view()}
            </div>
            {(!others.is_empty()).then(|| view! {
                <ul class="adm-rules">
                    {others.into_iter().map(|r| {
                        let rid = r.id;
                        view! {
                            <li>
                                <code class="mono">{r.path.clone()}</code>
                                <span class="faint small grow">{client::perms_words(&r.perms)}</span>
                                <button class="btn sm ghost" on:click=move |_| kit_leptos::rt::spawn(async move {
                                    match client::delete_rule(rid).await { Ok(()) => reload.run(()), Err(e) => fail(e) }
                                })>"Remove"</button>
                            </li>
                        }
                    }).collect_view()}
                </ul>
            })}
            <div class="adm-actions">
                <button class="btn sm primary" disabled=move || !dirty() || busy.get() on:click=save>{format!("Save {}", source.name)}</button>
            </div>
        </div>
    }
}

/// Groups: tick the groups the account is in.
#[component]
fn Groups(id: i64) -> impl IntoView {
    let all = LocalResource::new(|| async { client::groups().await.map_err(|e| e.message) });
    let mine = LocalResource::new(move || async move { client::user_groups(id).await.map_err(|e| e.message) });
    let toggle = move |g: i64, on: bool| {
        kit_leptos::rt::spawn(async move {
            let r = if on { client::add_member(g, id).await } else { client::remove_member(g, id).await };
            if let Err(e) = r {
                fail(e);
            }
            mine.refetch();
        })
    };
    view! {
        <section class="adm-section" aria-labelledby="sec-groups">
            <h3 id="sec-groups">"Groups"</h3>
            {move || match (all.get(), mine.get()) {
                (Some(Err(e)), _) | (_, Some(Err(e))) => failed(e, move || { all.refetch(); mine.refetch(); }),
                (Some(Ok(gs)), Some(Ok(_))) if gs.is_empty() => view! { <p class="faint small">"No groups yet. Make them on the Groups page; rules for a group cover every member."</p> }.into_any(),
                (Some(Ok(gs)), Some(Ok(m))) => view! {
                    <div class="adm-perms">
                        {gs.into_iter().map(|g| {
                            let on = m.iter().any(|x| x.id == g.id);
                            let gid = g.id;
                            view! {
                                <label class="adm-perm" class:on=on>
                                    <input type="checkbox" prop:checked=on on:change=move |e| toggle(gid, event_target_checked(&e))/>
                                    {g.name.clone()}
                                </label>
                            }
                        }).collect_view()}
                    </div>
                }.into_any(),
                _ => loading(),
            }}
        </section>
    }
}

/// Storage quota: the use, the limit box, Save and Count again.
#[component]
fn QuotaSection(id: i64) -> impl IntoView {
    let quota = LocalResource::new(move || async move { client::quota(id).await.map_err(|e| e.message) });
    let limit = RwSignal::new(None::<String>);
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let shown = move || limit.get().unwrap_or_else(|| client::size_box(quota.get().and_then(Result::ok).and_then(|q| q.limit)));
    let save = move |_| {
        let bytes = match client::parse_size(&shown()) {
            Ok(b) => b,
            Err(e) => {
                error.set(Some(e.into()));
                return;
            }
        };
        busy.set(true);
        error.set(None);
        kit_leptos::rt::spawn(async move {
            match client::set_quota(id, bytes).await {
                Ok(_) => {
                    limit.set(None);
                    quota.refetch();
                    done("Quota saved.");
                }
                Err(e) => error.set(Some(e.message)),
            }
            busy.set(false);
        });
    };
    let rescan = move |_| {
        kit_leptos::rt::spawn(async move {
            match client::rescan_quota(id).await {
                Ok(()) => {
                    quota.refetch();
                    done("Counting their storage again.");
                }
                Err(e) => fail(e),
            }
        })
    };
    view! {
        <section class="adm-section" aria-labelledby="sec-quota">
            <h3 id="sec-quota">"Storage quota"</h3>
            {move || match quota.get() {
                None => loading(),
                Some(Ok(q)) => view! { <QuotaBar q=q/> }.into_any(),
                Some(Err(e)) => failed(e, move || quota.refetch()),
            }}
            <label class="field"><span>"Limit"</span>
                <span class="adm-pw">
                    <input class="input" placeholder="no limit" prop:value=shown on:input=move |e| limit.set(Some(event_target_value(&e)))/>
                    <button class="btn primary" type="button" disabled=move || busy.get() || limit.get().is_none() on:click=save>"Save"</button>
                    <button class="btn" type="button" title="Count their storage again" on:click=rescan><span inner_html=ic("refresh")></span>"Recount"</button>
                </span>
                <small>"For example 50G. Empty means no limit."</small>
            </label>
            <p class="field-err small" role="alert" class:hidden=move || error.get().is_none()>{move || error.get()}</p>
        </section>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acct(id: i64, name: &str, role: Role) -> Account {
        Account { id, username: name.into(), auth_source: "local".into(), role, disabled: false, created_at: 0 }
    }

    fn rule(source: &str, subject: Subject, perms: &[&str]) -> Rule {
        Rule { id: 1, source: source.into(), path: "/".into(), subject, perms: perms.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn summaries_name_the_sources_once() {
        let rules = [
            rule("a", Subject::User(2), &["view"]),
            rule("a", Subject::User(2), &["view", "upload"]),
            rule("b", Subject::Group(2), &["view"]),
            rule("c", Subject::User(2), &[]),
        ];
        let name = |id: &str| id.to_uppercase();
        assert_eq!(access_summary(&acct(2, "bo", Role::User), &rules, name), "A");
        assert_eq!(access_summary(&acct(1, "ad", Role::Admin), &rules, name), "Every source");
        assert!(access_summary(&acct(3, "cy", Role::User), &rules, name).starts_with("No access"));
    }

    #[test]
    fn search_finds_names_and_roles() {
        let a = acct(1, "Ada", Role::ReadOnly);
        assert!(matches(&a, "ad"));
        assert!(matches(&a, "read"));
        assert!(matches(&a, "  "));
        assert!(!matches(&a, "zed"));
    }
}
