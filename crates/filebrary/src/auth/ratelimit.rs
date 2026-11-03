//! In-memory limiter for failed logins.
//!
//! Failures are counted per username and per client IP in a sliding window.
//! When either key reaches its limit, further attempts with that key are
//! refused until the oldest failure in the window expires. A successful
//! login clears the username's counter (not the IP's, so one valid account
//! cannot be used to reset guessing against others).

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::error::AppError;

/// Most keys tracked before stale entries are swept.
const SWEEP_THRESHOLD: usize = 10_000;

/// Counts failed logins and decides whether another attempt is allowed.
#[derive(Debug)]
pub struct LoginRateLimiter {
    window: Duration,
    max_per_user: u32,
    max_per_ip: u32,
    failures: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl LoginRateLimiter {
    /// Creates a limiter with the given window and per-key limits.
    pub fn new(window: Duration, max_per_user: u32, max_per_ip: u32) -> Self {
        Self {
            window,
            max_per_user,
            max_per_ip,
            failures: Mutex::new(HashMap::new()),
        }
    }

    fn keys(username: &str, ip: &str) -> (String, String) {
        (format!("u:{}", username.to_lowercase()), format!("ip:{ip}"))
    }

    /// Returns `Ok` if a login for `username` from `ip` may be attempted now.
    ///
    /// # Errors
    /// [`AppError::TooManyRequests`] with the seconds to wait.
    pub fn check(&self, username: &str, ip: &str) -> Result<(), AppError> {
        self.check_at(username, ip, Instant::now())
    }

    pub(crate) fn check_at(&self, username: &str, ip: &str, now: Instant) -> Result<(), AppError> {
        let (uk, ik) = Self::keys(username, ip);
        let mut map = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        let mut wait = Duration::ZERO;
        for (key, max) in [(uk, self.max_per_user), (ik, self.max_per_ip)] {
            if let Some(q) = map.get_mut(&key) {
                prune(q, now, self.window);
                if max > 0
                    && q.len() >= max as usize
                    && let Some(oldest) = q.front()
                {
                    wait = wait.max(self.window.saturating_sub(now - *oldest));
                }
            }
        }
        if wait.is_zero() {
            Ok(())
        } else {
            Err(AppError::TooManyRequests {
                retry_after: wait.as_secs().max(1),
            })
        }
    }

    /// Records a failed login for both keys.
    pub fn record_failure(&self, username: &str, ip: &str) {
        self.record_failure_at(username, ip, Instant::now());
    }

    pub(crate) fn record_failure_at(&self, username: &str, ip: &str, now: Instant) {
        let (uk, ik) = Self::keys(username, ip);
        let mut map = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        if map.len() > SWEEP_THRESHOLD {
            let window = self.window;
            map.retain(|_, q| {
                prune(q, now, window);
                !q.is_empty()
            });
        }
        for key in [uk, ik] {
            let q = map.entry(key).or_default();
            q.push_back(now);
            // Never keep more entries than the larger limit needs.
            let cap = self.max_per_user.max(self.max_per_ip).max(1) as usize;
            while q.len() > cap {
                q.pop_front();
            }
        }
    }

    /// Clears the failure count for `username` after a successful login.
    pub fn record_success(&self, username: &str) {
        let (uk, _) = Self::keys(username, "");
        self.failures
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&uk);
    }
}

fn prune(q: &mut VecDeque<Instant>, now: Instant, window: Duration) {
    while q.front().is_some_and(|t| now.duration_since(*t) >= window) {
        q.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locks_after_user_limit_and_recovers() {
        let rl = LoginRateLimiter::new(Duration::from_secs(60), 3, 100);
        let t0 = Instant::now();
        for _ in 0..3 {
            assert!(rl.check_at("Alice", "1.1.1.1", t0).is_ok());
            rl.record_failure_at("alice", "1.1.1.1", t0);
        }
        // Same user from another IP is still locked; usernames are case-insensitive.
        assert!(matches!(
            rl.check_at("ALICE", "2.2.2.2", t0),
            Err(AppError::TooManyRequests { .. })
        ));
        // Another user from the same IP is fine.
        assert!(rl.check_at("bob", "1.1.1.1", t0).is_ok());
        // After the window it unlocks.
        assert!(
            rl.check_at("alice", "1.1.1.1", t0 + Duration::from_secs(61))
                .is_ok()
        );
    }

    #[test]
    fn locks_after_ip_limit() {
        let rl = LoginRateLimiter::new(Duration::from_secs(60), 100, 2);
        let t0 = Instant::now();
        rl.record_failure_at("a", "9.9.9.9", t0);
        rl.record_failure_at("b", "9.9.9.9", t0);
        assert!(rl.check_at("c", "9.9.9.9", t0).is_err());
        assert!(rl.check_at("c", "8.8.8.8", t0).is_ok());
    }

    #[test]
    fn success_clears_user_counter() {
        let rl = LoginRateLimiter::new(Duration::from_secs(60), 2, 100);
        let t0 = Instant::now();
        rl.record_failure_at("a", "1", t0);
        rl.record_failure_at("a", "1", t0);
        assert!(rl.check_at("a", "1", t0).is_err());
        rl.record_success("a");
        assert!(rl.check_at("a", "1", t0).is_ok());
    }

    #[test]
    fn retry_after_counts_down() {
        let rl = LoginRateLimiter::new(Duration::from_secs(60), 1, 100);
        let t0 = Instant::now();
        rl.record_failure_at("a", "1", t0);
        match rl.check_at("a", "1", t0 + Duration::from_secs(20)) {
            Err(AppError::TooManyRequests { retry_after }) => assert_eq!(retry_after, 40),
            other => panic!("unexpected {other:?}"),
        }
    }
}
