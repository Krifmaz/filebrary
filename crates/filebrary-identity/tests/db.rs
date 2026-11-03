//! Integration tests against a fresh in-memory SQLite database with the
//! workspace's real migrations.

use std::str::FromStr;

use filebrary_identity::{Error, apikey, identities, quota, totp};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");
const KEY: [u8; 32] = [9; 32];

async fn db() -> SqlitePool {
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")
        .unwrap()
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .unwrap();
    MIGRATOR.run(&pool).await.unwrap();
    for (id, name) in [(1, "alice"), (2, "bob")] {
        sqlx::query(
            "INSERT INTO users (id, username, password_hash, created_at) VALUES (?, ?, 'x', 0)",
        )
        .bind(id)
        .bind(name)
        .execute(&pool)
        .await
        .unwrap();
    }
    pool
}

fn now() -> u64 {
    time::OffsetDateTime::now_utc().unix_timestamp() as u64
}

fn secret_of(e: &totp::Enrollment) -> Vec<u8> {
    filebrary_identity::base32::decode(&e.secret).unwrap()
}

#[tokio::test]
async fn totp_enrollment_login_and_replay() {
    let db = db().await;
    assert!(!totp::status(&db, 1).await.unwrap().enabled);
    let e = totp::begin_enrollment(&db, &KEY, 1, "Filebrary", "alice")
        .await
        .unwrap();
    assert!(e.uri.starts_with("otpauth://totp/Filebrary:alice?"));
    let secret = secret_of(&e);

    // Not active until confirmed.
    assert!(matches!(
        totp::verify_login(&db, &KEY, 1, &totp::code_at(&secret, now())).await,
        Err(Error::NotFound)
    ));
    assert!(
        matches!(
            totp::confirm_enrollment(&db, &KEY, 1, "000000").await,
            Err(Error::InvalidCredentials)
        ) || totp::code_at(&secret, now()) == "000000"
    );

    // Confirm with the previous step's code so the current one is still fresh.
    let prev = totp::code_at(&secret, now() - totp::STEP_SECS);
    let codes = totp::confirm_enrollment(&db, &KEY, 1, &prev).await.unwrap();
    assert_eq!(codes.len(), totp::RECOVERY_CODES);
    let st = totp::status(&db, 1).await.unwrap();
    assert!(st.enabled);
    assert_eq!(st.recovery_codes_left, 10);

    // Re-enrolling while on is refused.
    assert!(matches!(
        totp::begin_enrollment(&db, &KEY, 1, "F", "a").await,
        Err(Error::Conflict(_))
    ));

    // Login code works once, then is a replay.
    let cur = totp::code_at(&secret, now());
    totp::verify_login(&db, &KEY, 1, &cur).await.unwrap();
    assert!(matches!(
        totp::verify_login(&db, &KEY, 1, &cur).await,
        Err(Error::InvalidCredentials)
    ));
    // The confirmation code (an older step) is refused too.
    assert!(totp::verify_login(&db, &KEY, 1, &prev).await.is_err());

    // Recovery code: works once, any case and dashes.
    let rc = codes[3].to_uppercase().replace('-', " ");
    totp::verify_login(&db, &KEY, 1, &rc).await.unwrap();
    assert!(totp::verify_login(&db, &KEY, 1, &rc).await.is_err());
    assert_eq!(totp::status(&db, 1).await.unwrap().recovery_codes_left, 9);

    // Another user's codes are useless here.
    assert!(totp::verify_login(&db, &KEY, 2, &codes[0]).await.is_err());

    // The secret is not stored in clear.
    let stored: Vec<u8> = sqlx::query_scalar("SELECT secret_enc FROM user_totp WHERE user_id = 1")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_ne!(stored, secret);

    let fresh = totp::regenerate_recovery_codes(&db, 1).await.unwrap();
    assert!(totp::verify_login(&db, &KEY, 1, &codes[0]).await.is_err());
    totp::verify_login(&db, &KEY, 1, &fresh[0]).await.unwrap();

    totp::disable(&db, 1).await.unwrap();
    assert_eq!(totp::status(&db, 1).await.unwrap(), totp::Status::default());
}

#[tokio::test]
async fn api_keys_lifecycle() {
    let db = db().await;
    let scopes = apikey::parse_scopes(&["files:read"]).unwrap();
    let k = apikey::create(&db, 1, false, "backup script", &scopes, None)
        .await
        .unwrap();
    assert!(k.token.starts_with("fbk_"));

    let (uid, sc) = apikey::resolve(&db, &k.token).await.unwrap().unwrap();
    assert_eq!(uid, 1);
    assert_eq!(sc, [apikey::Scope::FilesRead]);
    assert!(
        apikey::list(&db, 1).await.unwrap()[0]
            .last_used_at
            .is_some()
    );

    // Not a Filebrary key: let other schemes try.
    assert!(
        apikey::resolve(&db, "eyJhbGciOi...")
            .await
            .unwrap()
            .is_none()
    );

    // Right id, wrong secret.
    let mut forged = k.token.clone();
    let last = forged.pop().unwrap();
    forged.push(if last == 'a' { 'b' } else { 'a' });
    assert!(matches!(
        apikey::resolve(&db, &forged).await,
        Err(Error::InvalidCredentials)
    ));

    // Admin scope needs an admin owner.
    assert!(
        apikey::create(&db, 2, false, "x", &[apikey::Scope::Admin], None)
            .await
            .is_err()
    );
    apikey::create(&db, 2, true, "x", &[apikey::Scope::Admin], None)
        .await
        .unwrap();

    // Expiry in the past is refused; an expired key stops working.
    assert!(
        apikey::create(&db, 1, false, "old", &scopes, Some(1))
            .await
            .is_err()
    );
    let short = apikey::create(&db, 1, false, "short", &scopes, Some(now() as i64 + 100))
        .await
        .unwrap();
    sqlx::query("UPDATE api_keys SET expires_at = 1 WHERE id = ?")
        .bind(&short.info.id)
        .execute(&db)
        .await
        .unwrap();
    assert!(apikey::resolve(&db, &short.token).await.is_err());
    assert_eq!(apikey::purge_expired(&db, 0).await.unwrap(), 1);

    // Users cannot revoke each other's keys; admins (None) can.
    assert!(matches!(
        apikey::revoke(&db, Some(2), &k.info.id).await,
        Err(Error::NotFound)
    ));
    apikey::revoke(&db, Some(1), &k.info.id).await.unwrap();
    assert!(apikey::resolve(&db, &k.token).await.is_err());

    // Secrets never appear in listings.
    let json = serde_json::to_string(&apikey::list(&db, 2).await.unwrap()).unwrap();
    assert!(!json.contains("secret"));
    assert!(json.contains("\"scopes\":[\"admin\"]"));
}

#[tokio::test]
async fn quotas_reserve_atomically() {
    let db = db().await;
    assert_eq!(quota::get(&db, 1).await.unwrap(), quota::Quota::default());
    // Unlimited users can always reserve.
    quota::reserve(&db, 1, 1 << 40).await.unwrap();
    quota::set_used(&db, 1, 0).await.unwrap();

    quota::set_limit(&db, 1, Some(100)).await.unwrap();
    quota::reserve(&db, 1, 60).await.unwrap();
    let err = quota::reserve(&db, 1, 41).await.unwrap_err();
    assert!(matches!(
        err,
        Error::QuotaExceeded {
            limit: 100,
            used: 60,
            requested: 41
        }
    ));
    quota::reserve(&db, 1, 40).await.unwrap();
    quota::release(&db, 1, 500).await.unwrap();
    assert_eq!(quota::get(&db, 1).await.unwrap().used, 0);
    assert!(quota::set_limit(&db, 1, Some(-1)).await.is_err());
    assert_eq!(quota::limited_users(&db).await.unwrap(), [1]);
    quota::set_limit(&db, 1, None).await.unwrap();
    assert!(quota::limited_users(&db).await.unwrap().is_empty());
}

#[tokio::test]
async fn identities_link_and_cascade() {
    let db = db().await;
    let p = identities::Provider::Oidc;
    assert_eq!(identities::find(&db, p, "iss", "sub").await.unwrap(), None);
    identities::link(&db, p, "iss", "sub", 2).await.unwrap();
    assert_eq!(
        identities::find(&db, p, "iss", "sub").await.unwrap(),
        Some(2)
    );
    // Same subject at another issuer is a different person.
    assert_eq!(
        identities::find(&db, p, "other", "sub").await.unwrap(),
        None
    );
    sqlx::query("DELETE FROM users WHERE id = 2")
        .execute(&db)
        .await
        .unwrap();
    assert_eq!(identities::find(&db, p, "iss", "sub").await.unwrap(), None);
}
