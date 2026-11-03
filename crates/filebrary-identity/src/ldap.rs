//! Password checks against an LDAP directory (OpenLDAP, Active Directory,
//! FreeIPA, lldap, ...).
//!
//! The usual search-then-bind flow:
//!
//! 1. Connect (`ldaps://`, or `ldap://` with optional StartTLS).
//! 2. Bind as the service account (or anonymously if none is configured).
//! 3. Search `base_dn` with `user_filter`, where `{username}` is replaced by
//!    the login name escaped per RFC 4515. Exactly one entry must match.
//! 4. Bind as that entry's DN with the password the user typed.
//!
//! Empty passwords are refused before anything is sent: many servers treat a
//! simple bind with an empty password as an anonymous bind and report
//! success, which would let anyone in.
//!
//! TLS uses rustls with the system's root certificates.

use std::{collections::HashMap, path::PathBuf, time::Duration};

use ldap3::{LdapConnAsync, LdapConnSettings, Scope, SearchEntry};
use serde::Deserialize;

use crate::{Error, Result};

/// LDAP result code for a wrong password or unknown DN.
const INVALID_CREDENTIALS: u32 = 49;

/// LDAP settings (`[auth.ldap]` in the config file).
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LdapConfig {
    /// Server URL: `ldaps://host:636` or `ldap://host:389`.
    pub url: String,
    /// Upgrade an `ldap://` connection with StartTLS.
    #[serde(default)]
    pub starttls: bool,
    /// DN of the service account used to search for users. Unset: anonymous search.
    #[serde(default)]
    pub bind_dn: Option<String>,
    /// Password of the service account. Prefer `bind_password_file`.
    #[serde(default)]
    pub bind_password: Option<crate::SecretString>,
    /// File holding the service account password (for Docker secrets).
    #[serde(default)]
    pub bind_password_file: Option<PathBuf>,
    /// Where to search for users, e.g. `ou=people,dc=example,dc=com`.
    pub base_dn: String,
    /// Search filter; `{username}` is replaced by the escaped login name.
    #[serde(default = "default_filter")]
    pub user_filter: String,
    /// Attribute holding the login name stored locally.
    #[serde(default = "default_username_attr")]
    pub username_attribute: String,
    /// Attribute holding the e-mail address.
    #[serde(default = "default_email_attr")]
    pub email_attribute: String,
    /// Attribute holding the display name.
    #[serde(default = "default_name_attr")]
    pub display_name_attribute: String,
    /// Attribute listing the groups a user belongs to.
    #[serde(default = "default_group_attr")]
    pub group_attribute: String,
    /// Only members of this group (full DN) may log in. Unset: anyone found.
    #[serde(default)]
    pub required_group: Option<String>,
    /// Members of this group (full DN) are administrators. Checked at every login.
    #[serde(default)]
    pub admin_group: Option<String>,
    /// Network timeout in seconds.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_filter() -> String {
    "(&(objectClass=person)(uid={username}))".into()
}
fn default_username_attr() -> String {
    "uid".into()
}
fn default_email_attr() -> String {
    "mail".into()
}
fn default_name_attr() -> String {
    "cn".into()
}
fn default_group_attr() -> String {
    "memberOf".into()
}
fn default_timeout() -> u64 {
    10
}

impl LdapConfig {
    /// Checks the settings without connecting.
    ///
    /// # Errors
    ///
    /// `InvalidInput` naming the bad setting.
    pub fn validate(&self) -> Result<()> {
        if !(self.url.starts_with("ldap://") || self.url.starts_with("ldaps://")) {
            return Err(Error::invalid(
                "ldap.url must start with ldap:// or ldaps://",
            ));
        }
        if self.starttls && self.url.starts_with("ldaps://") {
            return Err(Error::invalid("ldap.starttls only applies to ldap:// URLs"));
        }
        if !self.user_filter.contains("{username}") {
            return Err(Error::invalid("ldap.user_filter must contain {username}"));
        }
        if self.base_dn.trim().is_empty() {
            return Err(Error::invalid("ldap.base_dn is required"));
        }
        if self.bind_password.is_some() && self.bind_password_file.is_some() {
            return Err(Error::invalid(
                "set ldap.bind_password or ldap.bind_password_file, not both",
            ));
        }
        Ok(())
    }

    /// Whether traffic to the server is encrypted.
    pub fn is_encrypted(&self) -> bool {
        self.url.starts_with("ldaps://") || self.starttls
    }

    fn service_password(&self) -> Result<Option<String>> {
        crate::secret::read(
            self.bind_password.as_ref(),
            self.bind_password_file.as_deref(),
        )
        .map_err(|e| Error::Ldap(format!("reading ldap.bind_password_file: {e}")))
    }

    /// The filter for `username`, with the name escaped per RFC 4515.
    pub fn filter_for(&self, username: &str) -> String {
        self.user_filter
            .replace("{username}", &escape_filter_value(username))
    }
}

/// Escapes a value for use inside an LDAP search filter (RFC 4515 section 3):
/// `*`, `(`, `)`, `\` and NUL become `\2a`, `\28`, `\29`, `\5c` and `\00`.
pub fn escape_filter_value(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for c in v.chars() {
        match c {
            '*' => out.push_str("\\2a"),
            '(' => out.push_str("\\28"),
            ')' => out.push_str("\\29"),
            '\\' => out.push_str("\\5c"),
            '\0' => out.push_str("\\00"),
            c => out.push(c),
        }
    }
    out
}

/// A user the directory vouched for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LdapUser {
    /// The entry's DN (the stable link to the local user).
    pub dn: String,
    /// Login name from `username_attribute` (falls back to what was typed).
    pub username: String,
    /// E-mail address, if any.
    pub email: Option<String>,
    /// Display name, if any.
    pub display_name: Option<String>,
    /// Group DNs from `group_attribute`.
    pub groups: Vec<String>,
    /// Member of `admin_group`.
    pub is_admin: bool,
}

/// Checks `username` and `password` against the directory.
///
/// Returns `Ok(None)` for an unknown user, a wrong password, or a user outside
/// `required_group`; these are deliberately indistinguishable to the caller.
///
/// # Errors
///
/// `Ldap` when the server cannot be reached or the service bind fails.
pub async fn authenticate(
    cfg: &LdapConfig,
    username: &str,
    password: &str,
) -> Result<Option<LdapUser>> {
    if password.is_empty()
        || username.is_empty()
        || username.chars().count() > 256
        || username.chars().any(char::is_control)
    {
        return Ok(None);
    }
    let settings = LdapConnSettings::new()
        .set_conn_timeout(Duration::from_secs(cfg.timeout_secs))
        .set_starttls(cfg.starttls);
    let (conn, mut ldap) = LdapConnAsync::with_settings(settings, &cfg.url)
        .await
        .map_err(|e| Error::Ldap(format!("connecting to {}: {e}", cfg.url)))?;
    ldap3::drive!(conn);
    ldap.with_timeout(Duration::from_secs(cfg.timeout_secs));

    let result = async {
        if let Some(dn) = &cfg.bind_dn {
            let pw = cfg.service_password()?.unwrap_or_default();
            ldap.simple_bind(dn, &pw)
                .await
                .and_then(|r| r.success())
                .map_err(|e| Error::Ldap(format!("service bind failed: {e}")))?;
        }
        let attrs = [
            cfg.username_attribute.as_str(),
            cfg.email_attribute.as_str(),
            cfg.display_name_attribute.as_str(),
            cfg.group_attribute.as_str(),
        ];
        ldap.with_timeout(Duration::from_secs(cfg.timeout_secs));
        let (entries, _) = ldap
            .search(
                &cfg.base_dn,
                Scope::Subtree,
                &cfg.filter_for(username),
                attrs,
            )
            .await
            .and_then(|r| r.success())
            .map_err(|e| Error::Ldap(format!("search failed: {e}")))?;
        if entries.len() != 1 {
            if entries.len() > 1 {
                tracing::warn!(
                    count = entries.len(),
                    "LDAP filter matched several entries; refusing login"
                );
            }
            return Ok(None);
        }
        let entry = SearchEntry::construct(entries.into_iter().next().expect("one entry"));
        let user = to_user(cfg, username, &entry);
        if let Some(req) = &cfg.required_group
            && !in_group(&entry.attrs, &cfg.group_attribute, req)
        {
            return Ok(None);
        }
        ldap.with_timeout(Duration::from_secs(cfg.timeout_secs));
        let bind = ldap
            .simple_bind(&entry.dn, password)
            .await
            .map_err(|e| Error::Ldap(format!("user bind: {e}")))?;
        match bind.rc {
            0 => Ok(Some(user)),
            INVALID_CREDENTIALS => Ok(None),
            rc => Err(Error::Ldap(format!(
                "user bind returned code {rc}: {}",
                bind.text
            ))),
        }
    }
    .await;
    let _ = ldap.unbind().await;
    result
}

fn first_attr(attrs: &HashMap<String, Vec<String>>, name: &str) -> Option<String> {
    attrs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .and_then(|(_, v)| v.first().cloned())
        .filter(|s| !s.is_empty())
}

fn in_group(attrs: &HashMap<String, Vec<String>>, attr: &str, group_dn: &str) -> bool {
    let norm = |s: &str| s.replace(' ', "").to_ascii_lowercase();
    let want = norm(group_dn);
    attrs
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case(attr))
        .flat_map(|(_, v)| v.iter())
        .any(|g| norm(g) == want)
}

fn to_user(cfg: &LdapConfig, typed: &str, entry: &SearchEntry) -> LdapUser {
    LdapUser {
        dn: entry.dn.clone(),
        username: first_attr(&entry.attrs, &cfg.username_attribute)
            .unwrap_or_else(|| typed.to_owned()),
        email: first_attr(&entry.attrs, &cfg.email_attribute),
        display_name: first_attr(&entry.attrs, &cfg.display_name_attribute),
        groups: entry
            .attrs
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case(&cfg.group_attribute))
            .flat_map(|(_, v)| v.iter().cloned())
            .collect(),
        is_admin: cfg
            .admin_group
            .as_deref()
            .is_some_and(|g| in_group(&entry.attrs, &cfg.group_attribute, g)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> LdapConfig {
        toml_like(
            r#"{"url":"ldap://localhost:3890","base_dn":"ou=people,dc=example,dc=com",
                "admin_group":"cn=admins,ou=groups,dc=example,dc=com"}"#,
        )
    }

    fn toml_like(json: &str) -> LdapConfig {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn escaping_blocks_filter_injection() {
        assert_eq!(escape_filter_value("bob"), "bob");
        assert_eq!(escape_filter_value("*"), "\\2a");
        assert_eq!(
            escape_filter_value("a)(uid=*))(|(uid=b"),
            "a\\29\\28uid=\\2a\\29\\29\\28|\\28uid=b"
        );
        assert_eq!(escape_filter_value("x\\y\0"), "x\\5cy\\00");
        assert_eq!(
            cfg().filter_for("*)(cn=*"),
            "(&(objectClass=person)(uid=\\2a\\29\\28cn=\\2a))"
        );
    }

    #[test]
    fn validation() {
        assert!(cfg().validate().is_ok());
        let mut c = cfg();
        c.url = "http://x".into();
        assert!(c.validate().is_err());
        let mut c = cfg();
        c.user_filter = "(uid=bob)".into();
        assert!(c.validate().is_err());
        let mut c = cfg();
        c.url = "ldaps://x".into();
        c.starttls = true;
        assert!(c.validate().is_err());
        assert!(!cfg().is_encrypted());
    }

    #[test]
    fn maps_entry_and_groups() {
        let mut attrs = HashMap::new();
        attrs.insert("UID".to_owned(), vec!["bob".to_owned()]);
        attrs.insert("mail".to_owned(), vec!["bob@example.com".to_owned()]);
        attrs.insert(
            "memberOf".to_owned(),
            vec!["CN=admins, OU=groups,DC=example,DC=com".to_owned()],
        );
        let e = SearchEntry {
            dn: "uid=bob,ou=people,dc=example,dc=com".into(),
            attrs,
            bin_attrs: HashMap::new(),
        };
        let u = to_user(&cfg(), "BOB", &e);
        assert_eq!(u.username, "bob");
        assert_eq!(u.email.as_deref(), Some("bob@example.com"));
        assert_eq!(u.display_name, None);
        assert_eq!(u.groups.len(), 1);
        assert!(u.is_admin);
    }

    #[tokio::test]
    async fn empty_password_never_reaches_the_server() {
        // Port 1 is closed: if a connection were attempted this would be an error.
        let mut c = cfg();
        c.url = "ldap://127.0.0.1:1".into();
        assert_eq!(authenticate(&c, "bob", "").await.unwrap(), None);
        assert!(authenticate(&c, "bob", "pw").await.is_err());
    }
}
