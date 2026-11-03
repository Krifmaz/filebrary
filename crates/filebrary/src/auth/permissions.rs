//! Permission sets and per-folder permission rules.
//!
//! # Model
//!
//! A [`Rule`] grants a [`Perms`] set to one user or one group on a folder of
//! a source, and applies to that folder and everything below it. To find
//! what a user may do at a (real) path:
//!
//! 1. Administrators may do everything everywhere.
//! 2. Otherwise take the user's own rules and the rules of every group they
//!    belong to whose folder contains the path.
//! 3. Keep only the rules on the **most specific** (deepest) folder.
//! 4. If one of those is the user's own rule, it decides alone, so a user
//!    rule can narrow or widen what their groups get at that folder.
//!    Otherwise the group rules there are combined (union).
//! 5. With no matching rule the user may do nothing.
//!
//! A rule with an empty set therefore hides a subtree. Any permission
//! implies [`Perms::VIEW`].

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sqlx::SqlitePool;

use crate::{
    db,
    error::{AppError, AppResult},
    storage::VPath,
};

/// A set of permissions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Perms(u32);

impl Perms {
    /// No permissions.
    pub const NONE: Perms = Perms(0);
    /// See folders, file names, metadata and previews.
    pub const VIEW: Perms = Perms(1);
    /// Read file contents (download, stream, zip).
    pub const DOWNLOAD: Perms = Perms(1 << 1);
    /// Create new files and folders.
    pub const UPLOAD: Perms = Perms(1 << 2);
    /// Rename, move and overwrite.
    pub const EDIT: Perms = Perms(1 << 3);
    /// Delete (to the trash).
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

    /// Builds a set from raw bits. Unknown bits are dropped.
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

    /// Returns `true` for the empty set.
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Adds [`Perms::VIEW`] to a non-empty set (any permission implies view).
    #[must_use]
    pub fn with_implied(self) -> Self {
        if self.is_empty() {
            self
        } else {
            self | Self::VIEW
        }
    }

    /// The permission names in this set, in a fixed order.
    pub fn names(self) -> Vec<&'static str> {
        Self::NAMES
            .iter()
            .filter(|(_, p)| self.contains(*p))
            .map(|(n, _)| *n)
            .collect()
    }

    /// Parses permission names (`view`, `download`, `upload`, `edit`,
    /// `delete`, `share`).
    ///
    /// # Errors
    ///
    /// `BadRequest` on an unknown name.
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

impl<'de> Deserialize<'de> for Perms {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let names = Vec::<String>::deserialize(d)?;
        Perms::from_names(&names).map_err(serde::de::Error::custom)
    }
}

/// Who a rule applies to. JSON: `{"type": "user", "id": 3}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "type", content = "id")]
pub enum Subject {
    /// One user, by id.
    User(i64),
    /// Every member of a group, by id.
    Group(i64),
}

/// A stored permission rule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Rule {
    /// Database id.
    pub id: i64,
    /// Source name.
    pub source: String,
    /// The folder (real path in the source) the rule covers, with everything below.
    #[serde(serialize_with = "ser_vpath")]
    pub path: VPath,
    /// The user or group receiving the permissions.
    pub subject: Subject,
    /// The permissions granted.
    pub perms: Perms,
}

fn ser_vpath<S: Serializer>(p: &VPath, s: S) -> Result<S::Ok, S::Error> {
    s.collect_str(p)
}

#[derive(sqlx::FromRow)]
struct RuleRow {
    id: i64,
    source: String,
    path: String,
    user_id: Option<i64>,
    group_id: Option<i64>,
    perms: i64,
}

impl RuleRow {
    fn into_rule(self) -> Option<Rule> {
        let subject = match (self.user_id, self.group_id) {
            (Some(u), _) => Subject::User(u),
            (None, Some(g)) => Subject::Group(g),
            (None, None) => return None,
        };
        let Ok(path) = VPath::parse(&self.path) else {
            tracing::warn!(rule = self.id, "ignoring permission rule with invalid path");
            return None;
        };
        Some(Rule {
            id: self.id,
            source: self.source,
            path,
            subject,
            perms: Perms::from_bits(self.perms).with_implied(),
        })
    }
}

macro_rules! rule_cols {
    () => {
        "id, source, path, user_id, group_id, perms"
    };
}

/// Effective permissions at `path` from rules that already apply to the user
/// (their own and their groups'), following the model in the module docs.
/// Rules for other sources must be filtered out by the caller.
pub fn resolve<'a>(rules: impl IntoIterator<Item = &'a Rule>, path: &VPath) -> Perms {
    let mut best: Option<usize> = None;
    let mut user: Option<Perms> = None;
    let mut groups = Perms::NONE;
    for r in rules.into_iter().filter(|r| path.starts_with(&r.path)) {
        let depth = r.path.components().len();
        match best {
            Some(b) if depth < b => continue,
            Some(b) if depth == b => {}
            _ => {
                best = Some(depth);
                user = None;
                groups = Perms::NONE;
            }
        }
        match r.subject {
            Subject::User(_) => user = Some(user.unwrap_or_default() | r.perms),
            Subject::Group(_) => groups = groups | r.perms,
        }
    }
    user.unwrap_or(groups).with_implied()
}

/// Loads every rule that applies to `user_id` (own and via groups), on all sources.
///
/// # Errors
///
/// Database failures.
pub async fn rules_for_user(db: &SqlitePool, user_id: i64) -> AppResult<Vec<Rule>> {
    let rows: Vec<RuleRow> = sqlx::query_as(concat!(
        "SELECT ",
        rule_cols!(),
        " FROM permission_rules
         WHERE user_id = ?1
            OR group_id IN (SELECT group_id FROM group_members WHERE user_id = ?1)"
    ))
    .bind(user_id)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().filter_map(RuleRow::into_rule).collect())
}

/// Lists rules, optionally only those on one source, by source and path.
///
/// # Errors
///
/// Database failures.
pub async fn list(db: &SqlitePool, source: Option<&str>) -> AppResult<Vec<Rule>> {
    let rows: Vec<RuleRow> = sqlx::query_as(concat!(
        "SELECT ",
        rule_cols!(),
        " FROM permission_rules
         WHERE ?1 IS NULL OR source = ?1 ORDER BY source, path, id"
    ))
    .bind(source)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().filter_map(RuleRow::into_rule).collect())
}

/// Creates or replaces the rule for `subject` on (`source`, `path`).
///
/// # Errors
///
/// `NotFound` if the user or group does not exist; database failures.
pub async fn set(
    db: &SqlitePool,
    source: &str,
    path: &VPath,
    subject: Subject,
    perms: Perms,
) -> AppResult<Rule> {
    let (user_id, group_id) = match subject {
        Subject::User(id) => {
            super::users::get(db, id).await?;
            (Some(id), None)
        }
        Subject::Group(id) => {
            super::groups::get(db, id).await?;
            (None, Some(id))
        }
    };
    let path_s = path.to_string();
    let mut tx = db.begin().await?;
    sqlx::query(
        "DELETE FROM permission_rules
         WHERE source = ? AND path = ? AND user_id IS ? AND group_id IS ?",
    )
    .bind(source)
    .bind(&path_s)
    .bind(user_id)
    .bind(group_id)
    .execute(&mut *tx)
    .await?;
    let row: RuleRow = sqlx::query_as(concat!(
        "INSERT INTO permission_rules (source, path, user_id, group_id, perms, created_at)
         VALUES (?, ?, ?, ?, ?, ?) RETURNING ",
        rule_cols!()
    ))
    .bind(source)
    .bind(&path_s)
    .bind(user_id)
    .bind(group_id)
    .bind(perms.bits())
    .bind(db::now())
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    row.into_rule()
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("stored rule is invalid")))
}

/// Deletes a rule.
///
/// # Errors
///
/// `NotFound` if there is no such rule.
pub async fn delete(db: &SqlitePool, id: i64) -> AppResult<()> {
    let r = sqlx::query("DELETE FROM permission_rules WHERE id = ?")
        .bind(id)
        .execute(db)
        .await?;
    if r.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(path: &str, subject: Subject, perms: Perms) -> Rule {
        Rule {
            id: 0,
            source: "s".into(),
            path: VPath::parse(path).unwrap(),
            subject,
            perms: perms.with_implied(),
        }
    }

    fn p(s: &str) -> VPath {
        VPath::parse(s).unwrap()
    }

    #[test]
    fn names_round_trip_and_serde() {
        let x = Perms::from_names(&["view", "upload"]).unwrap();
        assert!(x.contains(Perms::VIEW) && !x.contains(Perms::DELETE));
        assert_eq!(x.names(), ["view", "upload"]);
        assert!(Perms::from_names(&["fly"]).is_err());
        assert_eq!(Perms::from_bits(Perms::ALL.bits() | 1 << 20), Perms::ALL);
        let y: Perms = serde_json::from_str(r#"["share"]"#).unwrap();
        assert_eq!(y, Perms::SHARE);
        assert!(serde_json::from_str::<Perms>(r#"["root"]"#).is_err());
        assert_eq!(
            serde_json::to_string(&y.with_implied()).unwrap(),
            r#"["view","share"]"#
        );
    }

    #[test]
    fn no_rules_means_nothing() {
        assert!(resolve(&[], &p("a")).is_empty());
    }

    #[test]
    fn most_specific_folder_wins() {
        let rules = [
            rule("/", Subject::Group(1), Perms::VIEW | Perms::DOWNLOAD),
            rule("/private", Subject::Group(1), Perms::NONE),
            rule("/drop", Subject::Group(1), Perms::UPLOAD),
        ];
        assert!(resolve(&rules, &p("docs/a.txt")).contains(Perms::DOWNLOAD));
        assert!(resolve(&rules, &p("private")).is_empty());
        assert!(resolve(&rules, &p("private/x/y")).is_empty());
        assert!(resolve(&rules, &p("privateer")).contains(Perms::DOWNLOAD));
        let drop = resolve(&rules, &p("drop/x"));
        assert!(drop.contains(Perms::UPLOAD | Perms::VIEW));
        assert!(!drop.contains(Perms::DOWNLOAD));
    }

    #[test]
    fn groups_combine_and_user_rule_decides_at_same_depth() {
        let mut rules = vec![
            rule("/team", Subject::Group(1), Perms::DOWNLOAD),
            rule("/team", Subject::Group(2), Perms::UPLOAD),
        ];
        assert!(resolve(&rules, &p("team/f")).contains(Perms::DOWNLOAD | Perms::UPLOAD));
        rules.push(rule("/team", Subject::User(7), Perms::VIEW));
        assert_eq!(resolve(&rules, &p("team/f")), Perms::VIEW);
    }

    #[test]
    fn deeper_group_rule_beats_shallower_user_rule() {
        let rules = [
            rule("/", Subject::User(7), Perms::VIEW | Perms::EDIT),
            rule("/ro", Subject::Group(1), Perms::VIEW),
        ];
        assert!(resolve(&rules, &p("x")).contains(Perms::EDIT));
        assert!(!resolve(&rules, &p("ro/x")).contains(Perms::EDIT));
    }
}
