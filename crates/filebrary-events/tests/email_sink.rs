//! Email delivery against a minimal in-process SMTP server.

use std::sync::{Arc, Mutex};

use filebrary_events::{Dispatcher, EmailConfig, NotificationsConfig};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Accepts one session and records the message data.
async fn smtp_server() -> (u16, Arc<Mutex<String>>) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let data = Arc::new(Mutex::new(String::new()));
    let d = data.clone();
    tokio::spawn(async move {
        let (sock, _) = l.accept().await.unwrap();
        let (r, mut w) = sock.into_split();
        let mut lines = BufReader::new(r).lines();
        w.write_all(b"220 test ESMTP\r\n").await.unwrap();
        let mut in_data = false;
        while let Ok(Some(line)) = lines.next_line().await {
            if in_data {
                if line == "." {
                    in_data = false;
                    w.write_all(b"250 queued\r\n").await.unwrap();
                } else {
                    let mut g = d.lock().unwrap();
                    g.push_str(&line);
                    g.push('\n');
                }
                continue;
            }
            let cmd = line.to_ascii_uppercase();
            let reply: &[u8] = if cmd.starts_with("EHLO") {
                b"250 test\r\n"
            } else if cmd.starts_with("DATA") {
                in_data = true;
                b"354 go\r\n"
            } else if cmd.starts_with("QUIT") {
                w.write_all(b"221 bye\r\n").await.unwrap();
                break;
            } else {
                b"250 ok\r\n"
            };
            w.write_all(reply).await.unwrap();
        }
    });
    (port, data)
}

#[tokio::test]
async fn sends_plain_text_mail() {
    let (port, data) = smtp_server().await;
    let mut c = NotificationsConfig {
        instance: "home".into(),
        ..Default::default()
    };
    c.email.push(EmailConfig {
        name: "mail".into(),
        smtp_host: "127.0.0.1".into(),
        smtp_port: Some(port),
        security: filebrary_events::EmailSecurity::None,
        username: None,
        password: None,
        from: "Filebrary <files@example.com>".into(),
        to: vec!["admin@example.com".into()],
        events: vec![],
    });
    let bus = filebrary_events::EventBus::new("home");
    let h = Dispatcher::from_config(&c).unwrap().spawn(&bus);
    h.send_test("mail", "the body".into()).await.unwrap();
    let got = data.lock().unwrap().clone();
    assert!(got.contains("Subject: [home] Filebrary test notification"), "{got}");
    assert!(got.contains("To: admin@example.com"), "{got}");
    assert!(got.contains("the body"), "{got}");
}
