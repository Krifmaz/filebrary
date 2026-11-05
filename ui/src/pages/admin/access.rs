//! `/admin/permissions`: every permission rule of a source, for users and groups, edited in place, with a
//! form for a new one; and `/admin/groups`: groups and their members.

use super::client::{self, Account, Group, Rule, Subject, PERMS};
use super::{done, empty, fail, failed, ic, loading};
use crate::api::{Role, Source};
use kit_rules::dialogs::Confirm;
use leptos::prelude::*;

type Sources = LocalResource<Result<Vec<Source>, String>>;

/// The name of a rule's subject ("Ada", "Group: Family"), from the accounts and groups.
pub fn subject_name(s: Subject, users: &[Account], groups: &[Group]) -> String {
    match s {
        Subject::User(id) => users.iter().find(|u| u.id == id).map(|u| u.username.clone()).unwrap_or_else(|| format!("Account {id}")),
        Subject::Group(id) => format!("Group: {}", groups.iter().find(|g| g.id == id).map(|g| g.name.as_str()).unwrap_or("(deleted)")),
    }
}

/// A subject as a picker value (`user:3`, `group:4`) and back.
pub fn subject_value(s: Subject) -> String {
    match s {
        Subject::User(id) => format!("user:{id}"),
        Subject::Group(id) => format!("group:{id}"),
    }
}

/// The subject of a picker value.
pub fn subject_of(v: &str) -> Option<Subject> {
    let (kind, id) = v.split_once(':')?;
    let id = id.parse().ok()?;
    match kind {
        "user" => Some(Subject::User(id)),
        "group" => Some(Subject::Group(id)),
        _ => None,
    }
}

/// A source's rules in the order the table shows them: by path, then groups before users, then by name.
pub fn sorted(rules: &[Rule], source: &str, users: &[Account], groups: &[Group]) -> Vec<Rule> {
    let mut v: Vec<Rule> = rules.iter().filter(|r| r.source == source).cloned().collect();
    v.sort_by_key(|r| (r.path.clone(), matches!(r.subject, Subject::User(_)), subject_name(r.subject, users, groups).to_lowercase()));
    v
}

/// The six permission boxes of one rule or form; `on_change` gets the new set.
#[component]
fn PermBoxes(perms: Signal<Vec<String>>, on_change: Callback<Vec<String>>, #[prop(into)] label: String) -> impl IntoView {
    view! {
        <div class="adm-perms" role="group" aria-label=label>
            {PERMS.iter().map(|p| {
                let p = *p;
                let on = move || perms.get().iter().any(|x| x == p);
                view! {
                    <label class="adm-perm" class:on=on>
                        <input type="checkbox" prop:checked=on on:change=move |e| on_change.run(client::toggle_perm(&perms.get_untracked(), p, event_target_checked(&e)))/>
                        {p}
                    </label>
                }
            }).collect_view()}
        </div>
    }
}

/// The Permissions page.
#[component]
pub fn PermissionsPage(sources: Sources) -> impl IntoView {
    let rules = LocalResource::new(|| async { client::rules().await.map_err(|e| e.message) });
    let users = LocalResource::new(|| async { client::users().await.unwrap_or_default() });
    let groups = LocalResource::new(|| async { client::groups().await.unwrap_or_default() });
    let picked = RwSignal::new(String::new());
    let source = move || {
        let list = sources.get().and_then(Result::ok).unwrap_or_default();
        let p = picked.get();
        list.iter().find(|s| s.id == p).or(list.first()).map(|s| s.id.clone()).unwrap_or_default()
    };

    let set = move |r: Rule, perms: Vec<String>| {
        kit_leptos::rt::spawn(async move {
            let res = if perms.is_empty() {
                client::delete_rule(r.id).await
            } else {
                client::set_rule(&r.source, &r.path, r.subject, &perms).await.map(drop)
            };
            match res {
                Ok(()) => rules.refetch(),
                Err(e) => fail(e),
            }
        })
    };
    let remove = move |r: Rule, who: String| {
        kit_leptos::rt::spawn(async move {
            let q = Confirm::new("Remove this rule?").text(format!("{who} loses what it gave on {}.", r.path)).ok("Remove").danger(true);
            if !kit_leptos::modal::confirm(q).await {
                return;
            }
            match client::delete_rule(r.id).await {
                Ok(()) => {
                    rules.refetch();
                    done("Rule removed.");
                }
                Err(e) => fail(e),
            }
        })
    };

    let picker = move || {
        let list = sources.get().and_then(Result::ok).unwrap_or_default();
        let now = source();
        view! {
            <label class="field adm-source-pick"><span>"Source"</span>
                <select class="input" on:change=move |e| picked.set(event_target_value(&e))>
                    {list.into_iter().map(|s| { let sel = s.id == now; view! { <option value=s.id.clone() selected=sel>{s.name}</option> } }).collect_view()}
                </select>
            </label>
        }
    };
    let table = move || {
        let rs = match rules.get() {
            None => return loading(),
            Some(Err(e)) => return failed(e, move || rules.refetch()),
            Some(Ok(rs)) => rs,
        };
        let (us, gs) = (users.get().unwrap_or_default(), groups.get().unwrap_or_default());
        let list = sorted(&rs, &source(), &us, &gs);
        if list.is_empty() {
            return empty("lock", "No rules on this source", "Only administrators reach it. Add a rule below to let a user or a group in.");
        }
        view! {
            <div class="table-wrap">
                <table class="table adm-rules-table">
                    <thead><tr><th>"Folder"</th><th>"Who"</th><th>"Permissions"</th><th></th></tr></thead>
                    <tbody>
                        {list.into_iter().map(|r| {
                            let who = subject_name(r.subject, &us, &gs);
                            let read_only = matches!(r.subject, Subject::User(id) if us.iter().any(|u| u.id == id && u.role == Role::ReadOnly));
                            let perms = Signal::stored(r.perms.clone());
                            let (r1, r2, who2) = (r.clone(), r.clone(), who.clone());
                            view! {
                                <tr>
                                    <td><code class="mono">{r.path.clone()}</code></td>
                                    <td><span class="ellipsis">{who.clone()}</span>{read_only.then(|| view! { <span class="badge info adm-cap">"read only"</span> })}</td>
                                    <td><PermBoxes perms=perms label=format!("Permissions of {who} on {}", r.path) on_change=Callback::new(move |p| set(r1.clone(), p))/></td>
                                    <td><button class="btn sm ghost icon" title="Remove" aria-label="Remove" on:click=move |_| remove(r2.clone(), who2.clone()) inner_html=ic("trash")></button></td>
                                </tr>
                            }
                        }).collect_view()}
                    </tbody>
                </table>
            </div>
        }
        .into_any()
    };

    // the new-rule form
    let who = RwSignal::new(String::new());
    let path = RwSignal::new(String::new());
    let new_perms = RwSignal::new(vec!["view".to_string(), "download".to_string()]);
    let busy = RwSignal::new(false);
    let add = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        let Some(subject) = subject_of(&who.get_untracked()) else {
            kit_leptos::toasts::toast("Pick who the rule is for.", "warn");
            return;
        };
        let src = source();
        let p = Some(path.get_untracked().trim().to_string()).filter(|p| !p.is_empty()).unwrap_or_else(|| "/".into());
        busy.set(true);
        kit_leptos::rt::spawn(async move {
            match client::set_rule(&src, &p, subject, &new_perms.get_untracked()).await {
                Ok(_) => {
                    path.set(String::new());
                    rules.refetch();
                    done("Rule saved.");
                }
                Err(e) => fail(e),
            }
            busy.set(false);
        });
    };
    let options = move || {
        let (us, gs) = (users.get().unwrap_or_default(), groups.get().unwrap_or_default());
        view! {
            <option value="" selected=move || who.get().is_empty()>"Pick a user or group"</option>
            <optgroup label="Groups">
                {gs.into_iter().map(|g| view! { <option value=subject_value(Subject::Group(g.id))>{g.name}</option> }).collect_view()}
            </optgroup>
            <optgroup label="Users">
                {us.into_iter().filter(|u| u.role != Role::Admin).map(|u| view! { <option value=subject_value(Subject::User(u.id))>{u.username}</option> }).collect_view()}
            </optgroup>
        }
    };
    view! {
        <section class="card" id="rules-card">
            <div class="adm-toolbar">
                <div class="grow">
                    <h2>"Permissions per source"</h2>
                    <p class="muted small">"A rule gives a user or a group permissions on a folder and everything inside it. Administrators always have every permission."</p>
                </div>
                {picker}
            </div>
            {table}
        </section>
        <form class="card" id="new-rule" on:submit=add>
            <h2>"New rule"</h2>
            <div class="form-grid">
                <label class="field"><span>"For"</span>
                    <select class="input" on:change=move |e| who.set(event_target_value(&e))>{options}</select>
                </label>
                <label class="field"><span>"Folder"</span>
                    <input class="input mono" placeholder="/ (the whole source)" prop:value=move || path.get() on:input=move |e| path.set(event_target_value(&e))/>
                    <small>"A rule for the same folder and the same user or group replaces the old one."</small>
                </label>
            </div>
            <div class="mt"><PermBoxes perms=new_perms.into() label="Permissions of the new rule" on_change=Callback::new(move |p| new_perms.set(p))/></div>
            <div class="adm-actions">
                <button class="btn primary" type="submit" disabled=move || busy.get() || new_perms.get().is_empty()><span inner_html=ic("plus")></span>"Add rule"</button>
            </div>
        </form>
    }
}

/// The Groups page: make, delete, and change who is in each.
#[component]
pub fn GroupsPage() -> impl IntoView {
    let groups = LocalResource::new(|| async { client::groups().await.map_err(|e| e.message) });
    let users = LocalResource::new(|| async { client::users().await.unwrap_or_default() });
    let name = RwSignal::new(String::new());
    let busy = RwSignal::new(false);
    let create = move |e: leptos::ev::SubmitEvent| {
        e.prevent_default();
        busy.set(true);
        kit_leptos::rt::spawn(async move {
            match client::create_group(name.get_untracked().trim()).await {
                Ok(g) => {
                    name.set(String::new());
                    groups.refetch();
                    done(&format!("Made the group {}.", g.name));
                }
                Err(e) => fail(e),
            }
            busy.set(false);
        });
    };
    let list = move || {
        match groups.get() {
        None => loading(),
        Some(Err(e)) => failed(e, move || groups.refetch()),
        Some(Ok(gs)) if gs.is_empty() => empty("users", "No groups yet", "A group lets one rule cover several people, such as Family or Editors."),
        Some(Ok(gs)) => view! {
            <div class="stack">
                {gs.into_iter().map(|g| view! { <GroupCard group=g users=users on_deleted=Callback::new(move |_| groups.refetch())/> }).collect_view()}
            </div>
        }
        .into_any(),
    }
    };
    view! {
        <form class="card adm-toolbar" id="new-group" on:submit=create>
            <label class="field grow"><span>"New group"</span>
                <input class="input" required maxlength="64" placeholder="Family" prop:value=move || name.get() on:input=move |e| name.set(event_target_value(&e))/>
            </label>
            <button class="btn primary adm-end" type="submit" disabled=move || busy.get() || name.get().trim().is_empty()><span inner_html=ic("plus")></span>"Make group"</button>
        </form>
        {list}
    }
}

/// One group: its members (with Remove), Add member, and Delete.
#[component]
fn GroupCard(group: Group, users: LocalResource<Vec<Account>>, on_deleted: Callback<()>) -> impl IntoView {
    let gid = group.id;
    let members = LocalResource::new(move || async move { client::members(gid).await.map_err(|e| e.message) });
    let adding = RwSignal::new(String::new());
    let add = move |_| {
        let Ok(uid) = adding.get_untracked().parse::<i64>() else { return };
        kit_leptos::rt::spawn(async move {
            match client::add_member(gid, uid).await {
                Ok(()) => {
                    adding.set(String::new());
                    members.refetch();
                }
                Err(e) => fail(e),
            }
        })
    };
    let gname = group.name.clone();
    let delete = move |_| {
        let gname = gname.clone();
        kit_leptos::rt::spawn(async move {
            let q = Confirm::new(format!("Delete the group {gname}?"))
                .text("Its rules go too, so its members lose what the group gave them. The accounts stay.")
                .ok("Delete group")
                .danger(true);
            if !kit_leptos::modal::confirm(q).await {
                return;
            }
            match client::delete_group(gid).await {
                Ok(()) => {
                    done(&format!("Deleted {gname}."));
                    on_deleted.run(());
                }
                Err(e) => fail(e),
            }
        })
    };
    let body = move || match members.get() {
        None => loading(),
        Some(Err(e)) => failed(e, move || members.refetch()),
        Some(Ok(ms)) => {
            let outside: Vec<Account> = users.get().unwrap_or_default().into_iter().filter(|u| !ms.iter().any(|m| m.id == u.id)).collect();
            view! {
                {if ms.is_empty() {
                    view! { <p class="faint small">"Nobody is in this group yet."</p> }.into_any()
                } else {
                    view! {
                        <div class="row wrap adm-chips">
                            {ms.into_iter().map(|m| {
                                let uid = m.id;
                                view! {
                                    <span class="badge adm-member">{m.username.clone()}
                                        <button class="adm-x" title=format!("Take {} out", m.username) aria-label=format!("Take {} out", m.username)
                                            on:click=move |_| kit_leptos::rt::spawn(async move {
                                                match client::remove_member(gid, uid).await { Ok(()) => members.refetch(), Err(e) => fail(e) }
                                            })>"×"</button>
                                    </span>
                                }
                            }).collect_view()}
                        </div>
                    }.into_any()
                }}
                {(!outside.is_empty()).then(|| view! {
                    <div class="adm-pw mt">
                        <select class="input" aria-label="Add a member" on:change=move |e| adding.set(event_target_value(&e))>
                            <option value="" selected=move || adding.get().is_empty()>"Add a member…"</option>
                            {outside.into_iter().map(|u| view! { <option value=u.id.to_string()>{u.username}</option> }).collect_view()}
                        </select>
                        <button class="btn" disabled=move || adding.get().is_empty() on:click=add>"Add"</button>
                    </div>
                })}
            }
            .into_any()
        }
    };
    view! {
        <section class="card adm-group" data-group=group.id.to_string()>
            <div class="row between">
                <h3 class="ellipsis">{group.name.clone()}</h3>
                <button class="btn sm ghost" on:click=delete><span inner_html=ic("trash")></span>"Delete"</button>
            </div>
            {body}
        </section>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subjects_round_trip_through_the_picker() {
        for s in [Subject::User(3), Subject::Group(12)] {
            assert_eq!(subject_of(&subject_value(s)), Some(s));
        }
        assert_eq!(subject_of(""), None);
        assert_eq!(subject_of("team:1"), None);
    }

    #[test]
    fn rules_sort_by_folder_then_groups_first() {
        let users =
            vec![Account { id: 1, username: "ada".into(), auth_source: "local".into(), role: Role::User, disabled: false, created_at: 0 }];
        let groups = vec![Group { id: 9, name: "Family".into(), created_at: 0 }];
        let r = |id, path: &str, subject| Rule { id, source: "home".into(), path: path.into(), subject, perms: vec![] };
        let rules = [
            r(1, "/b", Subject::User(1)),
            r(2, "/a", Subject::User(1)),
            r(3, "/a", Subject::Group(9)),
            Rule { source: "other".into(), ..r(4, "/", Subject::User(1)) },
        ];
        let ids: Vec<i64> = sorted(&rules, "home", &users, &groups).iter().map(|r| r.id).collect();
        assert_eq!(ids, [3, 2, 1]);
        assert_eq!(subject_name(Subject::Group(9), &users, &groups), "Group: Family");
        assert_eq!(subject_name(Subject::User(7), &users, &groups), "Account 7");
    }
}
