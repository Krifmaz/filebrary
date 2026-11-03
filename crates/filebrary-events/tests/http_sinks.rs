//! Webhook and ntfy delivery against a local HTTP server.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Router,
    body::Bytes,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use filebrary_events::{
    Dispatcher, Event, EventBus, NotificationsConfig, NtfyConfig, WebhookConfig, sign_payload,
};

type Seen = Arc<Mutex<Vec<(HeaderMap, Bytes)>>>;

async fn serve(status: StatusCode) -> (String, Seen) {
    let seen: Seen = Arc::default();
    let s = seen.clone();
    let app = Router::new().fallback(post(move |h: HeaderMap, b: Bytes| {
        let s = s.clone();
        async move {
            s.lock().unwrap().push((h, b));
            status
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), seen)
}

async fn wait(seen: &Seen, n: usize) {
    for _ in 0..400 {
        if seen.lock().unwrap().len() >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("expected {n} requests, got {}", seen.lock().unwrap().len());
}

fn cfg() -> NotificationsConfig {
    let mut c = NotificationsConfig {
        instance: "home".into(),
        ..Default::default()
    };
    c.retry.initial_backoff_ms = 1;
    c
}

#[tokio::test]
async fn webhook_is_signed_and_filtered() {
    let (url, seen) = serve(StatusCode::NO_CONTENT).await;
    let mut c = cfg();
    c.webhooks.push(
        toml::from_str::<WebhookConfig>(&format!(
            "name='hook'\nurl='{url}/in'\nsecret='s3cret'\nevents=['upload']\nheaders={{X-Extra='1'}}"
        ))
        .unwrap(),
    );
    let bus = EventBus::new("home");
    let _h = Dispatcher::from_config(&c).unwrap().spawn(&bus);
    bus.publish(Event::JobFailed {
        job_id: "1".into(),
        kind: "convert".into(),
        user: None,
        error: "x".into(),
    });
    bus.publish(Event::Upload {
        user: "alice".into(),
        source: "files".into(),
        path: "/a.txt".into(),
        size: 3,
    });
    wait(&seen, 1).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let got = seen.lock().unwrap();
    assert_eq!(got.len(), 1, "job_failed must be filtered out");
    let (headers, body) = &got[0];
    assert_eq!(headers["x-filebrary-event"], "upload");
    assert_eq!(headers["x-extra"], "1");
    let ts: i64 = headers["x-filebrary-timestamp"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        headers["x-filebrary-signature"].to_str().unwrap(),
        sign_payload(b"s3cret", ts, body)
    );
    let v: serde_json::Value = serde_json::from_slice(body).unwrap();
    assert_eq!(v["type"], "upload");
    assert_eq!(v["user"], "alice");
    assert_eq!(v["instance"], "home");
}

#[tokio::test]
async fn webhook_retries_server_errors() {
    let (url, seen) = serve(StatusCode::SERVICE_UNAVAILABLE).await;
    let mut c = cfg();
    c.retry.attempts = 3;
    c.webhooks
        .push(toml::from_str::<WebhookConfig>(&format!("name='hook'\nurl='{url}/'")).unwrap());
    let bus = EventBus::new("home");
    let h = Dispatcher::from_config(&c).unwrap().spawn(&bus);
    bus.publish(Event::Test {
        message: "x".into(),
    });
    wait(&seen, 3).await;
    // The admin test sends exactly once and reports the failure.
    let err = h.send_test("hook", "hi".into()).await.unwrap_err();
    assert!(err.contains("503"), "{err}");
}

#[tokio::test]
async fn ntfy_uses_json_publish_with_token() {
    let (url, seen) = serve(StatusCode::OK).await;
    let mut c = cfg();
    c.ntfy.push(
        toml::from_str::<NtfyConfig>(&format!(
            "name='phone'\nserver='{url}'\ntopic='files'\ntoken='tk_1'\nclick='https://files.example.com'"
        ))
        .unwrap(),
    );
    let bus = EventBus::new("home");
    let h = Dispatcher::from_config(&c).unwrap().spawn(&bus);
    h.send_test("phone", "héllo".into()).await.unwrap();
    let got = seen.lock().unwrap();
    let (headers, body) = &got[0];
    assert_eq!(headers["authorization"], "Bearer tk_1");
    let v: serde_json::Value = serde_json::from_slice(body).unwrap();
    assert_eq!(v["topic"], "files");
    assert_eq!(v["message"], "héllo");
    assert_eq!(v["click"], "https://files.example.com");
    assert!(v["title"].as_str().unwrap().starts_with("home: "));
}
