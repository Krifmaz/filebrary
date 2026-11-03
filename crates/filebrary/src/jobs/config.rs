//! Settings for the job system, read from the `[jobs]` table of the config
//! file and `FILEBRARY_JOB_*` / tool-path environment variables.
//!
//! ```toml
//! [jobs]
//! workers = 2
//! retain_days = 7
//! ffmpeg = "ffmpeg"
//! soffice = "soffice"
//!
//! # An admin-defined job. Users can run it on files; nobody can define or
//! # change it from the web UI or the API.
//! [[jobs.custom]]
//! name = "optimise-png"
//! title = "Optimise PNG"
//! program = "/usr/bin/optipng"
//! args = ["-quiet", "-o2", "{input}"]
//! extensions = ["png"]
//! output = "replace"
//! ```

use std::path::PathBuf;

use anyhow::bail;
use serde::{Deserialize, Serialize};

use crate::config::valid_source_name;

/// The `[jobs]` configuration table.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct JobsConfig {
    /// Jobs that run at the same time. Conversions are CPU heavy, so the
    /// default is low.
    pub workers: usize,
    /// Days to keep finished jobs (succeeded, failed or cancelled) before
    /// they are purged from the list.
    pub retain_days: u64,
    /// The ffmpeg program: a bare name looked up on `PATH`, or a full path.
    pub ffmpeg: String,
    /// The ffprobe program (used to read video durations for progress).
    pub ffprobe: String,
    /// The LibreOffice program used for document conversions.
    pub soffice: String,
    /// Longest a single external program may run, in seconds.
    pub timeout_secs: u64,
    /// Most files that one archive job may pack.
    pub max_archive_files: usize,
    /// Most entries listed when browsing inside an archive.
    pub max_archive_entries: usize,
    /// Largest single entry that can be read out of an archive, in MiB.
    pub max_archive_entry_mib: u64,
    /// Custom jobs, defined by the administrator in the config file only.
    pub custom: Vec<CustomJobConfig>,
}

impl Default for JobsConfig {
    fn default() -> Self {
        Self {
            workers: 2,
            retain_days: 7,
            ffmpeg: "ffmpeg".into(),
            ffprobe: "ffprobe".into(),
            soffice: "soffice".into(),
            timeout_secs: 6 * 3600,
            max_archive_files: 200_000,
            max_archive_entries: 100_000,
            max_archive_entry_mib: 4096,
            custom: Vec::new(),
        }
    }
}

/// What a custom job does with its results.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CustomOutput {
    /// Nothing is written back; the job's output text is kept in its result.
    #[default]
    None,
    /// The program changes `{input}` in place and the changed file replaces
    /// the original. Needs the edit permission.
    Replace,
    /// Every file the program writes into `{output_dir}` is uploaded next to
    /// the input file. Needs the upload permission.
    Files,
}

/// Which permission a user needs on the file to run a custom job.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CustomPermission {
    /// Read access (the default).
    #[default]
    Download,
    /// Upload access.
    Upload,
    /// Edit access.
    Edit,
}

/// One administrator-defined job.
///
/// The program is started directly with the given arguments: there is no
/// shell, so quotes, pipes, `$VAR` and globs have no special meaning. These
/// placeholders are replaced inside arguments:
///
/// * `{input}`: full path of a private temporary copy of the selected file;
/// * `{output_dir}`: an empty private folder for result files;
/// * `{ext}`: the input file's extension (lower case, letters and digits only).
///
/// The user's file name is never placed on the command line.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CustomJobConfig {
    /// Identifier, 1–32 of `[a-z0-9_-]`. The job kind is `custom.<name>`.
    pub name: String,
    /// Label shown in the UI.
    pub title: String,
    /// Longer help text shown in the UI.
    #[serde(default)]
    pub description: String,
    /// The program: a bare name looked up on `PATH`, or a full path.
    pub program: String,
    /// Arguments, with placeholders.
    #[serde(default)]
    pub args: Vec<String>,
    /// File extensions (without the dot) the job applies to. Empty: any file.
    #[serde(default)]
    pub extensions: Vec<String>,
    /// What happens to the results.
    #[serde(default)]
    pub output: CustomOutput,
    /// Permission needed on the file, besides what `output` needs.
    #[serde(default)]
    pub permission: CustomPermission,
    /// Only administrators may run this job.
    #[serde(default)]
    pub admin_only: bool,
    /// Time limit in seconds (defaults to `jobs.timeout_secs`).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// Extra environment variables for the program. The environment is
    /// otherwise cleared except for `PATH`, `HOME` and `LANG`.
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    /// Working directory. Defaults to the job's private temporary folder.
    #[serde(default)]
    pub workdir: Option<PathBuf>,
}

/// Placeholders a custom job argument may contain.
pub const PLACEHOLDERS: [&str; 3] = ["{input}", "{output_dir}", "{ext}"];

impl JobsConfig {
    /// Applies `FILEBRARY_JOB_WORKERS`, `FILEBRARY_JOB_RETAIN_DAYS`,
    /// `FILEBRARY_FFMPEG`, `FILEBRARY_FFPROBE` and `FILEBRARY_SOFFICE`.
    ///
    /// # Errors
    ///
    /// Fails on a non-numeric number.
    pub fn apply_env(&mut self, get: &impl Fn(&str) -> Option<String>) -> anyhow::Result<()> {
        if let Some(v) = get("FILEBRARY_JOB_WORKERS") {
            self.workers = v
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid value for FILEBRARY_JOB_WORKERS: {v:?}"))?;
        }
        if let Some(v) = get("FILEBRARY_JOB_RETAIN_DAYS") {
            self.retain_days = v.parse().map_err(|_| {
                anyhow::anyhow!("invalid value for FILEBRARY_JOB_RETAIN_DAYS: {v:?}")
            })?;
        }
        if let Some(v) = get("FILEBRARY_FFMPEG") {
            self.ffmpeg = v;
        }
        if let Some(v) = get("FILEBRARY_FFPROBE") {
            self.ffprobe = v;
        }
        if let Some(v) = get("FILEBRARY_SOFFICE") {
            self.soffice = v;
        }
        Ok(())
    }

    /// Checks the settings.
    ///
    /// # Errors
    ///
    /// Fails on out-of-range numbers, duplicate or invalid custom job names,
    /// empty programs and unknown placeholders.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.workers == 0 || self.workers > 64 {
            bail!("jobs.workers must be between 1 and 64");
        }
        if self.timeout_secs == 0 {
            bail!("jobs.timeout_secs must be at least 1");
        }
        let mut seen = std::collections::HashSet::new();
        for c in &self.custom {
            if !valid_source_name(&c.name) {
                bail!(
                    "invalid custom job name {:?}: use 1-32 characters from a-z, 0-9, '-' and '_'",
                    c.name
                );
            }
            if !seen.insert(&c.name) {
                bail!("duplicate custom job name {:?}", c.name);
            }
            if c.program.trim().is_empty() {
                bail!("custom job {:?} has no program", c.name);
            }
            for a in &c.args {
                check_placeholders(a).map_err(|p| {
                    anyhow::anyhow!("custom job {:?}: unknown placeholder {p}", c.name)
                })?;
            }
            if c.output == CustomOutput::Replace && !c.args.iter().any(|a| a.contains("{input}")) {
                bail!(
                    "custom job {:?} uses output = \"replace\" but never passes {{input}}",
                    c.name
                );
            }
            if c.output == CustomOutput::Files
                && !c.args.iter().any(|a| a.contains("{output_dir}"))
                && c.workdir.is_some()
            {
                bail!(
                    "custom job {:?} uses output = \"files\" with a workdir but never passes {{output_dir}}",
                    c.name
                );
            }
            for k in c.env.keys() {
                if k.is_empty() || k.contains('=') || k.contains('\0') {
                    bail!(
                        "custom job {:?}: invalid environment variable name {k:?}",
                        c.name
                    );
                }
            }
        }
        Ok(())
    }
}

/// Returns the first `{...}` token in `arg` that is not a known placeholder.
fn check_placeholders(arg: &str) -> Result<(), String> {
    let mut rest = arg;
    while let Some(start) = rest.find('{') {
        let Some(len) = rest[start..].find('}') else {
            return Ok(());
        };
        let token = &rest[start..start + len + 1];
        if !PLACEHOLDERS.contains(&token) {
            return Err(token.to_owned());
        }
        rest = &rest[start + len + 1..];
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(args: &[&str], output: CustomOutput) -> CustomJobConfig {
        CustomJobConfig {
            name: "x".into(),
            title: "X".into(),
            description: String::new(),
            program: "/bin/true".into(),
            args: args.iter().map(|s| (*s).to_owned()).collect(),
            extensions: vec![],
            output,
            permission: CustomPermission::Download,
            admin_only: false,
            timeout_secs: None,
            env: Default::default(),
            workdir: None,
        }
    }

    #[test]
    fn parses_custom_jobs() {
        let c: JobsConfig = toml::from_str(
            r#"
            workers = 3
            [[custom]]
            name = "opt"
            title = "Optimise"
            program = "optipng"
            args = ["-o2", "{input}"]
            extensions = ["png"]
            output = "replace"
            permission = "edit"
            "#,
        )
        .unwrap();
        assert_eq!(c.workers, 3);
        assert_eq!(c.custom[0].output, CustomOutput::Replace);
        assert_eq!(c.custom[0].permission, CustomPermission::Edit);
        c.validate().unwrap();
    }

    #[test]
    fn rejects_bad_custom_jobs() {
        let mut c = JobsConfig {
            custom: vec![job(&["{nope}"], CustomOutput::None)],
            ..JobsConfig::default()
        };
        assert!(c.validate().is_err());
        c.custom = vec![job(&["x"], CustomOutput::Replace)];
        assert!(c.validate().is_err());
        c.custom = vec![
            job(&["{input}"], CustomOutput::None),
            job(&[], CustomOutput::None),
        ];
        assert!(c.validate().is_err(), "duplicate names");
        c.custom = vec![job(
            &["--in={input}", "{output_dir}/{ext}"],
            CustomOutput::Files,
        )];
        c.validate().unwrap();
        c.custom[0].name = "Bad Name".into();
        assert!(c.validate().is_err());
    }

    #[test]
    fn env_overrides() {
        let mut c = JobsConfig::default();
        c.apply_env(&|k| match k {
            "FILEBRARY_JOB_WORKERS" => Some("4".into()),
            "FILEBRARY_FFMPEG" => Some("/opt/ffmpeg".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(c.workers, 4);
        assert_eq!(c.ffmpeg, "/opt/ffmpeg");
        assert!(
            JobsConfig::default()
                .apply_env(&|k| (k == "FILEBRARY_JOB_WORKERS").then(|| "many".into()))
                .is_err()
        );
    }
}
