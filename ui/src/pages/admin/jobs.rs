//! `/admin/jobs`: everyone's background jobs (cancel, retry, clear) and the search index of each source.

use super::client::{self, IndexStatus, Job};
use super::{done, empty, fail, failed, ic, loading};
use leptos::prelude::*;

/// The status filters: id ("" for all) and words.
pub const FILTERS: [(&str, &str); 6] =
    [("", "All"), ("running", "Running"), ("queued", "Queued"), ("failed", "Failed"), ("succeeded", "Done"), ("cancelled", "Cancelled")];

/// A job's status as a badge class and words.
pub fn status_badge(status: &str) -> (&'static str, &'static str) {
    match status {
        "queued" => ("badge", "Queued"),
        "running" => ("badge info", "Running"),
        "succeeded" => ("badge ok", "Done"),
        "failed" => ("badge err", "Failed"),
        "cancelled" => ("badge warn", "Cancelled"),
        _ => ("badge", "Unknown"),
    }
}

/// What can be done with a job in its state: (cancel, retry, clear).
pub fn actions(status: &str) -> (bool, bool, bool) {
    match status {
        "queued" | "running" => (true, false, false),
        "failed" | "cancelled" => (false, true, true),
        "succeeded" => (false, false, true),
        _ => (false, false, false),
    }
}

/// A scan's length in words ("0.4 s", "2 min 5 s").
pub fn took(ms: u64) -> String {
    match ms {
        0..=59_999 => format!("{:.1} s", ms as f64 / 1000.0),
        _ => format!("{} min {} s", ms / 60_000, (ms % 60_000) / 1000),
    }
}

/// The Jobs page.
#[component]
pub fn JobsPage() -> impl IntoView {
    let filter = RwSignal::new(String::new());
    let jobs = LocalResource::new(move || {
        let f = filter.get();
        async move { client::jobs(&f).await.map_err(|e| e.message) }
    });
    let index = LocalResource::new(|| async { client::index_status().await.map_err(|e| e.message) });
    // running jobs move: look again every few seconds while the page is open
    let alive = RwSignal::new(true);
    on_cleanup(move || alive.set(false));
    kit_leptos::rt::every(4000, move || {
        if alive.try_get_untracked().unwrap_or(false) {
            let busy = matches!(jobs.try_get_untracked().flatten(), Some(Ok(ref js)) if js.iter().any(|j| j.status == "running" || j.status == "queued"));
            let scanning = matches!(index.try_get_untracked().flatten(), Some(Ok(ref s)) if s.iter().any(|x| x.scanning));
            if busy {
                jobs.refetch();
            }
            if scanning {
                index.refetch();
            }
        }
    });

    let act = move |id: i64, what: &'static str| {
        kit_leptos::rt::spawn(async move {
            let r = match what {
                "cancel" => client::cancel_job(id).await,
                "retry" => client::retry_job(id).await,
                _ => client::delete_job(id).await,
            };
            match r {
                Ok(()) => jobs.refetch(),
                Err(e) => fail(e),
            }
        })
    };
    let list = move || match jobs.get() {
        None => loading(),
        Some(Err(e)) => failed(e, move || jobs.refetch()),
        Some(Ok(js)) if js.is_empty() => {
            empty("refresh", "No jobs here", "Conversions, archive work and scans show up here while they run.")
        }
        Some(Ok(js)) => view! { <ul class="list adm-list">{js.into_iter().map(|j| job_row(j, act)).collect_view()}</ul> }.into_any(),
    };
    let reindex_all = move |_| {
        kit_leptos::rt::spawn(async move {
            match client::reindex(None).await {
                Ok(()) => {
                    done("Scanning every source again.");
                    index.refetch();
                }
                Err(e) => fail(e),
            }
        })
    };
    let index_list = move || match index.get() {
        None => loading(),
        Some(Err(e)) => failed(e, move || index.refetch()),
        Some(Ok(s)) if s.is_empty() => {
            empty("search", "Search is off", "Turn the search index on in the server's settings to search every source.")
        }
        Some(Ok(s)) => view! { <ul class="list adm-list">{s.into_iter().map(|x| index_row(x, index)).collect_view()}</ul> }.into_any(),
    };
    view! {
        <section class="card" id="jobs-card">
            <div class="adm-toolbar">
                <div class="grow"><h2>"Jobs"</h2><p class="muted small">"Everyone's background work, newest first."</p></div>
                <button class="btn sm" on:click=move |_| jobs.refetch()><span inner_html=ic("refresh")></span>"Refresh"</button>
            </div>
            <div class="seg adm-filter" role="tablist" aria-label="Show jobs">
                {FILTERS.iter().map(|(id, words)| {
                    let id = *id;
                    view! { <button role="tab" class:on=move || filter.get() == id aria-selected=move || (filter.get() == id).to_string() on:click=move |_| filter.set(id.into())>{*words}</button> }
                }).collect_view()}
            </div>
            {list}
        </section>
        <section class="card" id="index-card">
            <div class="adm-toolbar">
                <div class="grow"><h2>"Search index"</h2><p class="muted small">"What search knows about each source. A scan reads every folder again."</p></div>
                <button class="btn sm" on:click=reindex_all><span inner_html=ic("refresh")></span>"Scan all"</button>
            </div>
            {index_list}
        </section>
    }
}

fn job_row(j: Job, act: impl Fn(i64, &'static str) + Copy + 'static) -> impl IntoView {
    let (cls, words) = status_badge(&j.status);
    let (cancel, retry, clear) = actions(&j.status);
    let pct = (j.progress.clamp(0.0, 1.0) * 100.0).round() as i64;
    let id = j.id;
    let title = if j.title.is_empty() { j.kind.clone() } else { j.title.clone() };
    let who = if j.user_id.is_none() { " · the server's own".to_string() } else { String::new() };
    let line = format!(
        "{}{} · {}{}",
        j.kind,
        who,
        client::date_time(j.created_at),
        if j.attempts > 1 { format!(" · try {}", j.attempts) } else { String::new() }
    );
    view! {
        <li class="list-item adm-job" data-job=id.to_string()>
            <div class="grow">
                <div class="row wrap"><strong class="ellipsis">{title}</strong><span class=cls>{words}</span></div>
                <span class="faint small">{line}</span>
                {(j.status == "running").then(|| view! {
                    <div class="adm-meter" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow=pct.to_string()><span style=format!("width:{pct}%")></span></div>
                })}
                {j.message.clone().filter(|m| !m.is_empty() && j.status == "running").map(|m| view! { <span class="muted small">{m}</span> })}
                {j.error.clone().map(|e| { let t = e.clone(); view! { <span class="field-err small ellipsis" data-details="" title=t>{e}</span> } })}
            </div>
            <div class="row">
                {cancel.then(|| view! { <button class="btn sm" on:click=move |_| act(id, "cancel")>"Cancel"</button> })}
                {retry.then(|| view! { <button class="btn sm" on:click=move |_| act(id, "retry")>"Retry"</button> })}
                {clear.then(|| view! { <button class="btn sm ghost icon" title="Clear from the list" aria-label="Clear from the list" on:click=move |_| act(id, "clear") inner_html=ic("x")></button> })}
            </div>
        </li>
    }
}

fn index_row(s: IndexStatus, index: LocalResource<Result<Vec<IndexStatus>, String>>) -> impl IntoView {
    let line = match (s.last_scan, s.last_scan_ms) {
        _ if s.scanning => "Scanning now…".to_string(),
        (Some(t), Some(ms)) => format!("{} entries · last scan {} ({})", s.entries, client::date_time(t), took(ms)),
        (Some(t), None) => format!("{} entries · last scan {}", s.entries, client::date_time(t)),
        _ => format!("{} entries · not scanned yet", s.entries),
    };
    let src = s.source.clone();
    let scan = move |_| {
        let src = src.clone();
        kit_leptos::rt::spawn(async move {
            match client::reindex(Some(&src)).await {
                Ok(()) => {
                    done(&format!("Scanning {src} again."));
                    index.refetch();
                }
                Err(e) => fail(e),
            }
        })
    };
    view! {
        <li class="list-item">
            <span class=if s.last_error.is_some() { "dot err" } else if s.scanning { "dot info" } else { "dot ok" }></span>
            <div class="grow">
                <div class="row wrap"><strong>{s.source.clone()}</strong>{s.watching.then(|| view! { <span class="badge">"kept fresh"</span> })}</div>
                <span class="faint small">{line}</span>
                {s.last_error.clone().map(|e| { let t = e.clone(); view! { <span class="field-err small ellipsis" data-details="" title=t>{e}</span> } })}
            </div>
            <button class="btn sm" disabled=s.scanning on:click=scan>"Scan"</button>
        </li>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_actions_follow_the_state() {
        assert_eq!(actions("running"), (true, false, false));
        assert_eq!(actions("failed"), (false, true, true));
        assert_eq!(actions("succeeded"), (false, false, true));
        for (id, _) in FILTERS.iter().skip(1) {
            assert_ne!(status_badge(id).1, "Unknown", "{id}");
        }
        assert_eq!(took(420), "0.4 s");
        assert_eq!(took(125_000), "2 min 5 s");
    }
}
