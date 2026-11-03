//! Brute-force protection for share passwords.
//!
//! Wrong passwords are counted per share in a fixed window. Once a share hits
//! the limit (the login limits from the config are reused), further unlock
//! attempts are refused until the window ends, whoever sends them. The table
//! lives in memory: a restart resets it, which is acceptable for a throttle.

use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

/// Failure count and window start (unix seconds) per share token.
static FAILURES: OnceLock<Mutex<HashMap<String, (u32, i64)>>> = OnceLock::new();

fn table() -> std::sync::MutexGuard<'static, HashMap<String, (u32, i64)>> {
    FAILURES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Returns the seconds to wait if `share` is currently locked out.
pub fn check(share: &str, max: u32, window: i64, now: i64) -> Option<u64> {
    let t = table();
    let (count, start) = *t.get(share)?;
    let end = start + window;
    (count >= max && now < end).then(|| u64::try_from(end - now).unwrap_or(1).max(1))
}

/// Records a failed attempt.
pub fn fail(share: &str, window: i64, now: i64) {
    let mut t = table();
    // Drop stale windows so the map cannot grow without bound.
    if t.len() > 10_000 {
        t.retain(|_, (_, start)| *start + window > now);
    }
    let e = t.entry(share.to_owned()).or_insert((0, now));
    if e.1 + window <= now {
        *e = (0, now);
    }
    e.0 += 1;
}

/// Forgets failures after a successful unlock.
pub fn clear(share: &str) {
    table().remove(share);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locks_after_max_and_resets() {
        let id = "ratelimit-unit-test";
        for _ in 0..3 {
            assert!(check(id, 3, 60, 1000).is_none());
            fail(id, 60, 1000);
        }
        assert_eq!(check(id, 3, 60, 1010), Some(50));
        assert!(check(id, 3, 60, 1060).is_none());
        fail(id, 60, 1061);
        assert!(check(id, 3, 60, 1061).is_none());
        clear(id);
        assert!(check(id, 3, 60, 1061).is_none());
    }
}
