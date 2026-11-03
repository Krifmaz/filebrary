//! Administrator-defined jobs (`custom.<name>`), configured only in the
//! config file under `[[jobs.custom]]` (see [`super::config`]). The web UI
//! and API can list and run them on a file but never create or change them.
//!
//! Parameters: `{"source": "files", "path": "/photo.png"}`.

use std::{path::Path, sync::Arc};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    CustomJobConfig, CustomOutput, JobContext, JobError, JobHandler, JobResult, Prepared,
    config::CustomPermission,
    process::{Cmd, run, which},
    stage,
};
use crate::{
    auth::{Perms, User},
    error::{AppError, AppResult},
    state::AppState,
    storage::{Storage, VPath},
};

/// Handler for one configured custom job.
pub struct CustomJob(pub CustomJobConfig);

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Params {
    source: String,
    path: String,
}

/// Most result files a `files` job may produce.
const MAX_OUTPUT_FILES: usize = 100;

impl CustomJob {
    fn perms(&self) -> Perms {
        let base = match self.0.permission {
            CustomPermission::Download => Perms::DOWNLOAD,
            CustomPermission::Upload => Perms::DOWNLOAD | Perms::UPLOAD,
            CustomPermission::Edit => Perms::DOWNLOAD | Perms::EDIT,
        };
        match self.0.output {
            CustomOutput::None => base,
            CustomOutput::Replace => base | Perms::EDIT,
            CustomOutput::Files => base,
        }
    }

    fn matches(&self, name: &str) -> bool {
        self.0.extensions.is_empty() || {
            let ext = stage::safe_ext(name);
            self.0
                .extensions
                .iter()
                .any(|e| e.eq_ignore_ascii_case(&ext))
        }
    }

    /// Checks access to the file (and its folder for `files` output).
    async fn check(
        &self,
        state: &AppState,
        user: &User,
        p: &Params,
    ) -> AppResult<(Arc<dyn Storage>, VPath, VPath)> {
        let (storage, real) = stage::resolve(state, user, &p.source, &p.path, self.perms()).await?;
        let entry = storage.stat(&real).await?;
        if entry.is_dir() {
            return Err(AppError::bad_request("select a file, not a folder"));
        }
        if !self.matches(&entry.name) {
            return Err(AppError::bad_request(format!(
                "{} only works on {} files",
                self.0.title,
                self.0.extensions.join(", ")
            )));
        }
        let parent = real.parent().unwrap_or_default();
        if self.0.output == CustomOutput::Files {
            stage::resolve(state, user, &p.source, &parent.to_string(), Perms::UPLOAD).await?;
        }
        Ok((storage, real, parent))
    }
}

/// Replaces the placeholders in one argument.
fn expand(arg: &str, input: &Path, output_dir: &Path, ext: &str) -> std::ffi::OsString {
    // Paths are absolute and server-chosen, so they never start with '-'.
    let mut s = arg.to_owned();
    s = s.replace("{input}", &input.to_string_lossy());
    s = s.replace("{output_dir}", &output_dir.to_string_lossy());
    s = s.replace("{ext}", ext);
    s.into()
}

#[async_trait]
impl JobHandler for CustomJob {
    fn available(&self, _state: &AppState) -> Result<(), String> {
        which(&self.0.program)
            .map(drop)
            .ok_or_else(|| format!("{} is not installed on this server", self.0.program))
    }

    fn admin_only(&self) -> bool {
        self.0.admin_only
    }

    fn describe(&self) -> Value {
        json!({
            "title": self.0.title,
            "description": self.0.description,
            "extensions": self.0.extensions,
            "output": self.0.output,
            "custom": true,
        })
    }

    async fn prepare(&self, state: &AppState, user: &User, params: Value) -> AppResult<Prepared> {
        let p: Params = serde_json::from_value(params)
            .map_err(|e| AppError::bad_request(format!("invalid parameters: {e}")))?;
        let (_, real, _) = self.check(state, user, &p).await?;
        Ok(Prepared {
            title: format!("{}: {}", self.0.title, real.name().unwrap_or("file")),
            params: serde_json::to_value(&p).map_err(anyhow::Error::from)?,
        })
    }

    async fn run(&self, ctx: &JobContext, params: &Value) -> JobResult<Value> {
        let p: Params = serde_json::from_value(params.clone())
            .map_err(|e| JobError::failed(format!("invalid parameters: {e}")))?;
        let user = ctx.user().await?;
        if self.0.admin_only && !user.is_admin {
            return Err(JobError::failed("only administrators can run this job"));
        }
        let (storage, real, parent) = self.check(&ctx.state, &user, &p).await?;
        let program = which(&self.0.program)
            .ok_or_else(|| JobError::failed(format!("{} is not installed", self.0.program)))?;

        let name = real.name().unwrap_or("file").to_owned();
        let ext = stage::safe_ext(&name);
        let in_dir = ctx.tmp.join("in");
        let out_dir = ctx.tmp.join("out");
        let work = ctx.tmp.join("work");
        for d in [&in_dir, &out_dir, &work] {
            tokio::fs::create_dir_all(d).await?;
        }
        let input = in_dir.join(if ext.is_empty() {
            "input".to_owned()
        } else {
            format!("input.{ext}")
        });
        ctx.progress(0.0, Some("copying input")).await;
        stage::download(storage.as_ref(), &real, &input, &ctx.cancel, |_| {}).await?;
        ctx.progress(0.1, Some("running")).await;

        let timeout = std::time::Duration::from_secs(
            self.0
                .timeout_secs
                .unwrap_or(ctx.state.config.jobs.timeout_secs),
        );
        let mut cmd = Cmd::new(program, self.0.workdir.clone().unwrap_or(work), timeout).args(
            self.0
                .args
                .iter()
                .map(|a| expand(a, &input, &out_dir, &ext)),
        );
        cmd.env = self.0.env.clone();
        let out = run(&cmd, &ctx.cancel, |_| {}).await?;
        ctx.check_cancel()?;
        ctx.progress(0.9, Some("saving")).await;

        let writer = stage::Writer {
            state: &ctx.state,
            user: &user,
            source: &p.source,
        };
        let mut result = json!({ "output": out.stdout_tail });
        match self.0.output {
            CustomOutput::None => {}
            CustomOutput::Replace => {
                let bytes = tokio::fs::metadata(&input).await?.len();
                writer.check(Perms::EDIT, &real, bytes).await?;
                stage::upload(storage.as_ref(), &real, &input, true).await?;
                ctx.record_write(
                    &user,
                    &p.source,
                    &real,
                    crate::activity::action::EDIT,
                    "custom",
                )
                .await;
                result["path"] = json!(p.path);
            }
            CustomOutput::Files => {
                let mut written = Vec::new();
                let mut rd = tokio::fs::read_dir(&out_dir).await?;
                while let Some(e) = rd.next_entry().await? {
                    // Only regular files; symlinks could point anywhere.
                    if !e.file_type().await?.is_file() {
                        continue;
                    }
                    if written.len() >= MAX_OUTPUT_FILES {
                        return Err(JobError::failed("the job produced too many files"));
                    }
                    let fname = e.file_name().to_string_lossy().into_owned();
                    let (stem, fext) = stage::split_name(&fname);
                    let path = stage::upload_new(
                        &writer,
                        storage.as_ref(),
                        &parent,
                        stem,
                        fext,
                        &e.path(),
                    )
                    .await?;
                    ctx.record_write(
                        &user,
                        &p.source,
                        &path,
                        crate::activity::action::UPLOAD,
                        "custom",
                    )
                    .await;
                    written.push(path.to_string());
                }
                result["files"] = json!(written);
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_placeholders() {
        let a = expand(
            "--in={input}",
            Path::new("/t/in/input.png"),
            Path::new("/t/out"),
            "png",
        );
        assert_eq!(a, "--in=/t/in/input.png");
        let a = expand(
            "{output_dir}/x.{ext}",
            Path::new("/i"),
            Path::new("/t/out"),
            "png",
        );
        assert_eq!(a, "/t/out/x.png");
    }
}
