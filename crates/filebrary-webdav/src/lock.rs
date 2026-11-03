//! Write locks (WebDAV class 2, RFC 4918 §6 and §9.10).
//!
//! Locks live in memory: they are short-lived by design (clients refresh
//! them), and losing them on restart only means clients take them again.
//! They are keyed by source and *real* path, so a lock taken by one user
//! protects the file from every other user, whatever their scope.
//!
//! A lock is held by a principal (the [`crate::DavUser::id`]). Submitting a
//! lock token only counts when the request comes from the same principal, so
//! a token seen in a listing cannot be used by someone else.

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use filebrary_storage::VPath;

/// Default lock lifetime when the client does not ask for one.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);
/// Longest lock lifetime granted, whatever the client asks for (including
/// `Infinite`). Clients that need longer refresh the lock.
pub const MAX_TIMEOUT: Duration = Duration::from_secs(3600);
/// Most locks held at once across the server.
pub const MAX_LOCKS: usize = 10_000;

/// An active lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lock {
    /// The lock token (`opaquelocktoken:<uuid>`).
    pub token: String,
    /// The source the lock is on.
    pub source: String,
    /// The real path of the lock root.
    pub root: VPath,
    /// `Depth: infinity` (the lock covers everything below a collection).
    pub infinite: bool,
    /// Shared rather than exclusive.
    pub shared: bool,
    /// The client's `DAV:owner` element, serialized, if it sent one.
    pub owner: Option<String>,
    /// The user who holds the lock.
    pub principal: String,
    /// The lifetime granted.
    pub timeout: Duration,
    /// When the lock expires unless refreshed.
    pub expires: Instant,
}

impl Lock {
    /// True if the lock applies to `path` in `source`: it is the root, or the
    /// lock is infinite and the root is an ancestor.
    pub fn covers(&self, source: &str, path: &VPath) -> bool {
        self.source == source
            && (self.root == *path || (self.infinite && path.starts_with(&self.root)))
    }

    /// Seconds left before the lock expires.
    pub fn remaining(&self) -> Duration {
        self.expires.saturating_duration_since(Instant::now())
    }
}

/// Why a lock could not be taken, refreshed or released.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LockError {
    /// Another lock conflicts; holds the real roots of the conflicting locks
    /// (423 Locked).
    #[error("resource is locked")]
    Conflict(Vec<VPath>),
    /// No such token, or it does not cover the request URI (409 for UNLOCK,
    /// 412 for a refresh).
    #[error("lock token does not match the resource")]
    NoMatch,
    /// The token belongs to another user (403).
    #[error("lock is held by another user")]
    NotOwner,
    /// The server holds too many locks (503).
    #[error("too many locks")]
    TooMany,
}

/// How much of the tree a change touches, for [`LockManager::check`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Only the resource itself (PUT, PROPPATCH).
    Resource,
    /// The resource and everything below it (DELETE, MOVE source, COPY or
    /// MOVE overwriting a collection).
    Tree,
}

/// The lock table.
#[derive(Debug, Default)]
pub struct LockManager {
    locks: Mutex<HashMap<String, Lock>>,
}

/// Parses a `Timeout` header (`Second-600, Infinite`), returning the first
/// usable value, capped at [`MAX_TIMEOUT`].
pub fn parse_timeout(value: Option<&str>) -> Duration {
    let Some(v) = value else {
        return DEFAULT_TIMEOUT;
    };
    for part in v.split(',') {
        let part = part.trim();
        if part.eq_ignore_ascii_case("infinite") {
            return MAX_TIMEOUT;
        }
        if let Some(n) = part
            .get(..7)
            .filter(|p| p.eq_ignore_ascii_case("second-"))
            .and_then(|_| part[7..].parse::<u64>().ok())
        {
            return Duration::from_secs(n.clamp(1, MAX_TIMEOUT.as_secs()));
        }
    }
    DEFAULT_TIMEOUT
}

impl LockManager {
    /// An empty table.
    pub fn new() -> Self {
        Self::default()
    }

    fn table(&self) -> std::sync::MutexGuard<'_, HashMap<String, Lock>> {
        let mut t = self
            .locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        t.retain(|_, l| l.expires > now);
        t
    }

    /// Locks that apply to `path` (for `lockdiscovery`).
    pub fn discover(&self, source: &str, path: &VPath) -> Vec<Lock> {
        let mut v: Vec<Lock> = self
            .table()
            .values()
            .filter(|l| l.covers(source, path))
            .cloned()
            .collect();
        v.sort_by(|a, b| a.token.cmp(&b.token));
        v
    }

    /// The lock with this token, if it is active.
    pub fn get(&self, token: &str) -> Option<Lock> {
        self.table().get(token).cloned()
    }

    /// Takes a new lock.
    ///
    /// # Errors
    ///
    /// [`LockError::Conflict`] if an exclusive lock is involved on either side
    /// and the two locks overlap; [`LockError::TooMany`] when the table is
    /// full.
    #[allow(clippy::too_many_arguments)]
    pub fn lock(
        &self,
        source: &str,
        root: &VPath,
        infinite: bool,
        shared: bool,
        owner: Option<String>,
        principal: &str,
        timeout: Duration,
    ) -> Result<Lock, LockError> {
        let mut t = self.table();
        let conflicts: Vec<VPath> = t
            .values()
            .filter(|l| {
                let overlap = l.covers(source, root)
                    || (infinite && l.source == source && l.root.starts_with(root));
                overlap && !(l.shared && shared)
            })
            .map(|l| l.root.clone())
            .collect();
        if !conflicts.is_empty() {
            return Err(LockError::Conflict(conflicts));
        }
        if t.len() >= MAX_LOCKS {
            return Err(LockError::TooMany);
        }
        let lock = Lock {
            token: format!("opaquelocktoken:{}", uuid::Uuid::new_v4()),
            source: source.to_owned(),
            root: root.clone(),
            infinite,
            shared,
            owner,
            principal: principal.to_owned(),
            timeout,
            expires: Instant::now() + timeout,
        };
        t.insert(lock.token.clone(), lock.clone());
        Ok(lock)
    }

    /// Refreshes a lock that covers `path`.
    ///
    /// # Errors
    ///
    /// [`LockError::NoMatch`] if the token is unknown or does not cover the
    /// path, [`LockError::NotOwner`] if another user holds it.
    pub fn refresh(
        &self,
        token: &str,
        principal: &str,
        source: &str,
        path: &VPath,
        timeout: Duration,
    ) -> Result<Lock, LockError> {
        let mut t = self.table();
        let l = t.get_mut(token).ok_or(LockError::NoMatch)?;
        if !l.covers(source, path) {
            return Err(LockError::NoMatch);
        }
        if l.principal != principal {
            return Err(LockError::NotOwner);
        }
        l.timeout = timeout;
        l.expires = Instant::now() + timeout;
        Ok(l.clone())
    }

    /// Releases a lock that covers `path`.
    ///
    /// # Errors
    ///
    /// As for [`LockManager::refresh`].
    pub fn unlock(
        &self,
        token: &str,
        principal: &str,
        source: &str,
        path: &VPath,
    ) -> Result<(), LockError> {
        let mut t = self.table();
        let l = t.get(token).ok_or(LockError::NoMatch)?;
        if !l.covers(source, path) {
            return Err(LockError::NoMatch);
        }
        if l.principal != principal {
            return Err(LockError::NotOwner);
        }
        t.remove(token);
        Ok(())
    }

    /// Checks that a change to `path` is allowed: every lock that applies
    /// (and, for [`Scope::Tree`], every lock below the path) must have its
    /// token in `tokens` and belong to `principal`.
    ///
    /// # Errors
    ///
    /// [`LockError::Conflict`] with the roots of the locks not satisfied.
    pub fn check(
        &self,
        source: &str,
        path: &VPath,
        scope: Scope,
        tokens: &[String],
        principal: &str,
    ) -> Result<(), LockError> {
        let t = self.table();
        let mut blocked: Vec<VPath> = t
            .values()
            .filter(|l| {
                let applies = l.covers(source, path)
                    || (scope == Scope::Tree && l.source == source && l.root.starts_with(path));
                applies && !(l.principal == principal && tokens.contains(&l.token))
            })
            .map(|l| l.root.clone())
            .collect();
        if blocked.is_empty() {
            Ok(())
        } else {
            blocked.sort();
            blocked.dedup();
            Err(LockError::Conflict(blocked))
        }
    }

    /// Drops every lock rooted at or below `path` (after DELETE or MOVE).
    pub fn remove_tree(&self, source: &str, path: &VPath) {
        self.table()
            .retain(|_, l| !(l.source == source && l.root.starts_with(path)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> VPath {
        VPath::parse(s).unwrap()
    }

    const T: Duration = DEFAULT_TIMEOUT;

    #[test]
    fn timeouts() {
        assert_eq!(parse_timeout(None), DEFAULT_TIMEOUT);
        assert_eq!(parse_timeout(Some("Second-30")), Duration::from_secs(30));
        assert_eq!(parse_timeout(Some("Infinite, Second-30")), MAX_TIMEOUT);
        assert_eq!(parse_timeout(Some("Second-4100000000")), MAX_TIMEOUT);
        assert_eq!(parse_timeout(Some("bogus")), DEFAULT_TIMEOUT);
        assert_eq!(parse_timeout(Some("second-0")), Duration::from_secs(1));
    }

    #[test]
    fn exclusive_and_shared() {
        let m = LockManager::new();
        let a = m.lock("s", &p("f"), false, true, None, "u1", T).unwrap();
        let _b = m.lock("s", &p("f"), false, true, None, "u2", T).unwrap();
        assert!(matches!(
            m.lock("s", &p("f"), false, false, None, "u1", T),
            Err(LockError::Conflict(_))
        ));
        assert_eq!(m.discover("s", &p("f")).len(), 2);
        assert_eq!(
            m.unlock(&a.token, "u2", "s", &p("f")),
            Err(LockError::NotOwner)
        );
        assert_eq!(
            m.unlock(&a.token, "u1", "s", &p("g")),
            Err(LockError::NoMatch)
        );
        m.unlock(&a.token, "u1", "s", &p("f")).unwrap();
        assert_eq!(m.discover("s", &p("f")).len(), 1);
    }

    #[test]
    fn depth_and_check() {
        let m = LockManager::new();
        let d = m.lock("s", &p("d"), true, false, None, "u1", T).unwrap();
        // The child is covered by the infinite lock.
        assert_eq!(m.discover("s", &p("d/x/y")).len(), 1);
        assert!(m.lock("s", &p("d/x"), false, false, None, "u1", T).is_err());
        // An infinite lock on the parent conflicts with the existing one below.
        assert!(
            m.lock("s", &VPath::root(), true, false, None, "u1", T)
                .is_err()
        );
        // But a depth-0 lock on the parent does not.
        let r = m
            .lock("s", &VPath::root(), false, false, None, "u1", T)
            .unwrap();

        let none: Vec<String> = vec![];
        assert!(
            m.check("s", &p("d/x"), Scope::Resource, &none, "u1")
                .is_err()
        );
        assert!(
            m.check(
                "s",
                &p("d/x"),
                Scope::Resource,
                std::slice::from_ref(&d.token),
                "u1"
            )
            .is_ok()
        );
        // Same token from another user does not count.
        assert!(
            m.check(
                "s",
                &p("d/x"),
                Scope::Resource,
                std::slice::from_ref(&d.token),
                "u2"
            )
            .is_err()
        );
        // Deleting the root tree touches both locks.
        let both = vec![d.token.clone(), r.token.clone()];
        assert!(
            m.check("s", &VPath::root(), Scope::Tree, &both, "u1")
                .is_ok()
        );
        assert!(
            m.check(
                "s",
                &VPath::root(),
                Scope::Tree,
                std::slice::from_ref(&r.token),
                "u1"
            )
            .is_err()
        );
        // Other sources are unaffected.
        assert!(m.check("t", &p("d/x"), Scope::Tree, &none, "u2").is_ok());
        m.remove_tree("s", &p("d"));
        assert!(m.get(&d.token).is_none());
        assert!(m.get(&r.token).is_some());
    }

    #[test]
    fn refresh_and_expiry() {
        let m = LockManager::new();
        let l = m
            .lock(
                "s",
                &p("f"),
                false,
                false,
                None,
                "u",
                Duration::from_millis(1),
            )
            .unwrap();
        std::thread::sleep(Duration::from_millis(5));
        assert!(m.get(&l.token).is_none());
        assert_eq!(
            m.refresh(&l.token, "u", "s", &p("f"), T),
            Err(LockError::NoMatch)
        );
        let l = m.lock("s", &p("f"), false, false, None, "u", T).unwrap();
        let r = m.refresh(&l.token, "u", "s", &p("f"), MAX_TIMEOUT).unwrap();
        assert_eq!(r.timeout, MAX_TIMEOUT);
    }
}
