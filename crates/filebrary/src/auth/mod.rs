//! Users, permissions, sessions and the request extractors that enforce them.
//!
//! # Contract for handlers
//!
//! * Take [`AuthUser`] as an argument to require a logged-in user. The
//!   extractor validates the session cookie and, for unsafe methods (anything
//!   but GET/HEAD/OPTIONS), the `X-CSRF-Token` header.
//! * Take [`AdminUser`] to require an administrator.
//! * Call [`User::require`] with the source, the permission needed and the
//!   *user-visible* path. It returns the real path inside the source (the
//!   user's scope prefixed), or `NotFound`/`Forbidden`.
//!
//! Ownership: the login/logout/users/password endpoints, argon2id hashing,
//! rate limiting and admin bootstrap live in `password`, `ratelimit` and
//! `routes` and are owned by the Auth work stream.

pub mod password;
pub mod routes;
pub mod session;

use std::collections::BTreeMap;

use axum::{extract::FromRequestParts, http::request::Parts};
use axum_extra::extract::CookieJar;
use serde::{Serialize, Serializer};

use crate::{
    crypto,
    error::{AppError, AppResult},
    state::AppState,
    storage::VPath,
};

/// Name of the CSRF header clients send on unsafe requests.
pub const CSRF_HEADER: &str = "x-csrf-token";

/// A set of permissions on a source.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Perms(u32);

impl Perms {
    /// See folders and file names.
    pub const VIEW: Perms = Perms(1);
    /// Read file contents (download, preview, zip).
    pub const DOWNLOAD: Perms = Perms(1 << 1);
    /// Create new files and folders.
    pub const UPLOAD: Perms = Perms(1 << 2);
    /// Rename, move and overwrite.
    pub const EDIT: Perms = Perms(1 << 3);
    /// Delete.
    pub const DELETE: Perms = Perms(1 << 4);
    /// Create share links.
    pub const SHARE: Perms = Perms(1 << 5);
    /// Every permission.
    pub const ALL: Perms = Perms(0b11_1111);

    const NAMES: [(&'static str, Perms); 6] = [
        ("view", Self::VIEW),
        ("download", Self::DOWNLOAD),
        ("upload", Self::UPLOAD),
        ("edit", Self::EDIT),
        ("delete", Self::DELETE),
        ("share", Self::SHARE),
    ];

    /// Builds a set from raw bits (unknown bits are dropped).
    pub fn from_bits(bits: i64) -> Self {
        Perms((bits as u32) & Self::ALL.0)
    }

    /// The raw bits, for storage.
    pub fn bits(self) -> i64 {
        i64::from(self.0)
    }

    /// Returns `true` if every permission in `other` is in `self`.
    pub fn contains(self, other: Perms) -> bool {
        self.0 & other.0 == other.0
    }

    /// The permission names in this set.
    pub fn names(self) -> Vec<&'static str> {
        Self::NAMES
            .iter()
            .filter(|(_, p)| self.contains(*p))
            .map(|(n, _)| *n)
            .collect()
    }

    /// Parses permission names.
    ///
    /// # Errors
    ///
    /// Fails on an unknown name.
    pub fn from_names<S: AsRef<str>>(names: &[S]) -> AppResult<Self> {
        let mut bits = 0;
        for n in names {
            let (_, p) = Self::NAMES
                .iter()
                .find(|(name, _)| *name == n.as_ref())
                .ok_or_else(|| {
                    AppError::bad_request(format!("unknown permission {:?}", n.as_ref()))
                })?;
            bits |= p.0;
        }
        Ok(Perms(bits))
    }
}

impl std::ops::BitOr for Perms {
    type Output = Perms;
    fn bitor(self, rhs: Perms) -> Perms {
        Perms(self.0 | rhs.0)
    }
}

impl Serialize for Perms {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.names().serialize(s)
    }
}

/// A user's access to one source.
#[derive(Clone, Debug, Serialize)]
pub struct Grant {
    /// What the user may do.
    pub perms: Perms,
    /// The folder the user is confined to; paths they send are relative to it.
    pub scope: VPath,
}

/// A loaded user with their grants.
#[derive(Clone, Debug, Serialize)]
pub struct User {
    /// Database id.
    pub id: i64,
    /// Login name.
    pub username: String,
    /// Administrators have every permission on every source.
    pub is_admin: bool,
    /// Grants by source name (administrators get `ALL` on every source).
    pub grants: BTreeMap<String, Grant>,
}

impl User {
    /// The user's grant on `source`, if any.
    pub fn grant(&self, source: &str) -> Option<&Grant> {
        self.grants.get(source)
    }

    /// Checks that the user may do `perm` on `source` and maps the
    /// user-visible `path` to the real path inside the source.
    ///
    /// Sources the user cannot view are reported as `NotFound` so their
    /// existence is not revealed.
    ///
    /// # Errors
    ///
    /// `NotFound` without view access, `Forbidden` without `perm`.
    pub fn require(&self, source: &str, perm: Perms, path: &VPath) -> AppResult<VPath> {
        let g = self.grant(source).ok_or(AppError::NotFound)?;
        if !g.perms.contains(Perms::VIEW) {
            return Err(AppError::NotFound);
        }
        if !g.perms.contains(perm) {
            return Err(AppError::forbidden(format!(
                "you do not have the {} permission here",
                perm.names().join("+")
            )));
        }
        Ok(g.scope.join_path(path))
    }

    /// Maps a real source path back to what this user sees, if it is in scope.
    pub fn visible_path(&self, source: &str, real: &VPath) -> Option<VPath> {
        real.strip_prefix(&self.grant(source)?.scope)
    }
}

/// Loads an enabled user and their grants. Returns `None` if missing or disabled.
///
/// # Errors
///
/// Database failures.
pub async fn load_user(state: &AppState, id: i64) -> AppResult<Option<User>> {
    let row: Option<(i64, String, bool, bool)> =
        sqlx::query_as("SELECT id, username, is_admin, disabled FROM users WHERE id = ?")
            .bind(id)
            .fetch_optional(&state.db)
            .await?;
    let Some((id, username, is_admin, disabled)) = row else {
        return Ok(None);
    };
    if disabled {
        return Ok(None);
    }
    let mut grants = BTreeMap::new();
    if is_admin {
        for name in state.sources.keys() {
            grants.insert(
                name.clone(),
                Grant {
                    perms: Perms::ALL,
                    scope: VPath::root(),
                },
            );
        }
    } else {
        let rows: Vec<(String, i64, String)> =
            sqlx::query_as("SELECT source, perms, scope FROM user_sources WHERE user_id = ?")
                .bind(id)
                .fetch_all(&state.db)
                .await?;
        for (source, perms, scope) in rows {
            if !state.sources.contains_key(&source) {
                continue; // source removed from the config
            }
            let Ok(scope) = VPath::parse(&scope) else {
                tracing::warn!(user = id, %source, "ignoring grant with invalid scope");
                continue;
            };
            grants.insert(
                source,
                Grant {
                    perms: Perms::from_bits(perms),
                    scope,
                },
            );
        }
    }
    Ok(Some(User {
        id,
        username,
        is_admin,
        grants,
    }))
}

/// Extractor for a logged-in user (see the module docs for the CSRF rule).
pub struct AuthUser {
    /// The user.
    pub user: User,
    /// SHA-256 of the session token (to log out this session).
    pub session_hash: Vec<u8>,
    /// The session's CSRF token.
    pub csrf_token: String,
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> AppResult<Self> {
        let jar = CookieJar::from_headers(&parts.headers);
        let token = jar
            .get(session::SESSION_COOKIE)
            .map(|c| c.value().to_owned())
            .ok_or(AppError::Unauthorized)?;
        let sess = session::lookup(state, &token)
            .await?
            .ok_or(AppError::Unauthorized)?;
        if !parts.method.is_safe() {
            let sent = parts
                .headers
                .get(CSRF_HEADER)
                .map(|v| v.as_bytes())
                .unwrap_or_default();
            if !crypto::ct_eq(sent, sess.csrf_token.as_bytes()) {
                return Err(AppError::forbidden("missing or invalid CSRF token"));
            }
        }
        let user = load_user(state, sess.user_id)
            .await?
            .ok_or(AppError::Unauthorized)?;
        Ok(Self {
            user,
            session_hash: sess.token_hash,
            csrf_token: sess.csrf_token,
        })
    }
}

/// Extractor for a logged-in administrator.
pub struct AdminUser(pub AuthUser);

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> AppResult<Self> {
        let u = AuthUser::from_request_parts(parts, state).await?;
        if !u.user.is_admin {
            return Err(AppError::forbidden("administrators only"));
        }
        Ok(Self(u))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perms_names_round_trip() {
        let p = Perms::from_names(&["view", "upload"]).unwrap();
        assert!(p.contains(Perms::VIEW));
        assert!(!p.contains(Perms::DELETE));
        assert_eq!(p.names(), ["view", "upload"]);
        assert!(Perms::from_names(&["fly"]).is_err());
        assert_eq!(Perms::from_bits(Perms::ALL.bits() | 1 << 20), Perms::ALL);
    }

    #[test]
    fn require_maps_scope() {
        let mut grants = BTreeMap::new();
        grants.insert(
            "s".to_owned(),
            Grant {
                perms: Perms::VIEW | Perms::DOWNLOAD,
                scope: VPath::parse("home/bob").unwrap(),
            },
        );
        let u = User {
            id: 1,
            username: "bob".into(),
            is_admin: false,
            grants,
        };
        let real = u
            .require("s", Perms::DOWNLOAD, &VPath::parse("a.txt").unwrap())
            .unwrap();
        assert_eq!(real.to_string(), "/home/bob/a.txt");
        assert!(matches!(
            u.require("s", Perms::DELETE, &VPath::root()),
            Err(AppError::Forbidden(_))
        ));
        assert!(matches!(
            u.require("x", Perms::VIEW, &VPath::root()),
            Err(AppError::NotFound)
        ));
        assert_eq!(u.visible_path("s", &real).unwrap().to_string(), "/a.txt");
        assert!(
            u.visible_path("s", &VPath::parse("home/eve").unwrap())
                .is_none()
        );
    }
}
