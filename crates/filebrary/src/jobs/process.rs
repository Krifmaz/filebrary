//! Runs external programs (ffmpeg, LibreOffice, admin-defined tools) safely.
//!
//! * Programs are started directly with an argument vector. Nothing ever goes
//!   through a shell, so user data cannot be interpreted as shell syntax.
//! * The environment is cleared except for `PATH`, `HOME` (the job's private
//!   folder), `LANG` and variables the administrator configured.
//! * stdin is closed; stdout is handed line by line to a callback (ffmpeg
//!   progress); the last few KiB of stderr are kept for the error message.
//! * The program runs in its own process group (Unix) and the whole group is
//!   killed on cancel, on timeout and when the job is dropped, so helpers such
//!   as `soffice.bin` do not linger.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    process::Command,
};
use tokio_util::sync::CancellationToken;

use super::JobError;

/// Bytes of stderr (and of stdout, for [`Output::stdout_tail`]) kept.
const TAIL_BYTES: usize = 8 * 1024;

/// A program invocation.
#[derive(Debug, Clone)]
pub struct Cmd {
    /// The program, already resolved (see [`which`]).
    pub program: PathBuf,
    /// Arguments, passed as-is.
    pub args: Vec<OsString>,
    /// Working directory.
    pub cwd: PathBuf,
    /// Extra environment variables.
    pub env: BTreeMap<String, String>,
    /// Kill the program after this long.
    pub timeout: Duration,
}

impl Cmd {
    /// A command for `program` running in `cwd` with a time limit.
    pub fn new(program: impl Into<PathBuf>, cwd: impl Into<PathBuf>, timeout: Duration) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: cwd.into(),
            env: BTreeMap::new(),
            timeout,
        }
    }

    /// Appends one argument.
    pub fn arg(mut self, a: impl Into<OsString>) -> Self {
        self.args.push(a.into());
        self
    }

    /// Appends several arguments.
    pub fn args<I, S>(mut self, it: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(it.into_iter().map(Into::into));
        self
    }
}

/// What a finished program produced.
#[derive(Debug, Default)]
pub struct Output {
    /// The last few KiB of standard output.
    pub stdout_tail: String,
    /// The last few KiB of standard error.
    pub stderr_tail: String,
}

/// Runs `cmd` to completion.
///
/// `on_line` is called for every line the program writes to stdout. Returns
/// [`JobError::Cancelled`] if `cancel` fires, and [`JobError::Failed`] on a
/// non-zero exit (with the end of stderr as the message) or a timeout.
///
/// # Errors
///
/// See above; also fails if the program cannot be started.
pub async fn run(
    cmd: &Cmd,
    cancel: &CancellationToken,
    mut on_line: impl FnMut(&str) + Send,
) -> Result<Output, JobError> {
    let mut c = Command::new(&cmd.program);
    c.args(&cmd.args)
        .current_dir(&cmd.cwd)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", &cmd.cwd)
        .env("LANG", "C.UTF-8")
        .envs(&cmd.env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    if let Some(v) = std::env::var_os("SystemRoot") {
        c.env("SystemRoot", v);
    }
    #[cfg(unix)]
    c.process_group(0);

    let name = cmd
        .program
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut child = c
        .spawn()
        .map_err(|e| JobError::Failed(format!("cannot start {name}: {e}")))?;
    let pid = child.id();
    let _group = GroupGuard(pid);

    let stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let err_task = tokio::spawn(async move {
        let mut tail = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match stderr.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => push_tail(&mut tail, &buf[..n]),
            }
        }
        tail
    });

    let mut out_tail = Vec::new();
    let mut lines = BufReader::new(stdout).lines();
    let work = async {
        while let Ok(Some(line)) = lines.next_line().await {
            on_line(&line);
            push_tail(&mut out_tail, line.as_bytes());
            push_tail(&mut out_tail, b"\n");
        }
        child.wait().await
    };

    let status = tokio::select! {
        r = work => r.map_err(|e| JobError::Failed(format!("{name}: {e}")))?,
        () = cancel.cancelled() => return Err(JobError::Cancelled),
        () = tokio::time::sleep(cmd.timeout) => {
            return Err(JobError::Failed(format!(
                "{name} took longer than {} seconds and was stopped",
                cmd.timeout.as_secs()
            )));
        }
    };
    let err_tail = err_task.await.unwrap_or_default();
    let out = Output {
        stdout_tail: String::from_utf8_lossy(&out_tail).into_owned(),
        stderr_tail: String::from_utf8_lossy(&err_tail).trim().to_owned(),
    };
    if !status.success() {
        let detail = out
            .stderr_tail
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .to_owned();
        return Err(JobError::Failed(match status.code() {
            Some(code) => format!("{name} exited with status {code}: {detail}"),
            None => format!("{name} was killed: {detail}"),
        }));
    }
    Ok(out)
}

fn push_tail(tail: &mut Vec<u8>, data: &[u8]) {
    tail.extend_from_slice(data);
    if tail.len() > TAIL_BYTES {
        let cut = tail.len() - TAIL_BYTES;
        tail.drain(..cut);
    }
}

/// Kills the child's whole process group when dropped (Unix). The child
/// itself is also killed by `kill_on_drop`.
struct GroupGuard(Option<u32>);

impl Drop for GroupGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.0.and_then(|p| i32::try_from(p).ok())
            && pid > 0
        {
            // SAFETY: kill(2) with a negative pid signals the process group we
            // created with process_group(0); it has no memory-safety effects.
            // If the group is already gone this fails with ESRCH, which is fine.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
        #[cfg(not(unix))]
        let _ = self.0;
    }
}

/// Finds a program: a value containing a path separator is used as given
/// (and must exist); a bare name is searched for on `PATH`.
pub fn which(program: &str) -> Option<PathBuf> {
    let p = Path::new(program);
    if p.components().count() > 1 || p.is_absolute() {
        return p.is_file().then(|| p.to_path_buf());
    }
    let path = std::env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT".into())
            .split(';')
            .map(str::to_owned)
            .chain(std::iter::once(String::new()))
            .collect()
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(&path) {
        for ext in &exts {
            let cand = dir.join(format!("{program}{ext}"));
            if is_executable(&cand) {
                return Some(cand);
            }
        }
    }
    None
}

fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn sh() -> PathBuf {
        which("sh").expect("sh on PATH")
    }

    #[tokio::test]
    async fn runs_and_collects_lines() {
        let dir = tempfile::tempdir().unwrap();
        let cmd = Cmd::new(sh(), dir.path(), Duration::from_secs(10))
            .args(["-c", "echo one; echo two; echo oops >&2"]);
        let mut lines = Vec::new();
        let out = run(&cmd, &CancellationToken::new(), |l| {
            lines.push(l.to_owned())
        })
        .await
        .unwrap();
        assert_eq!(lines, ["one", "two"]);
        assert_eq!(out.stderr_tail, "oops");
    }

    #[tokio::test]
    async fn arguments_are_not_interpreted() {
        let dir = tempfile::tempdir().unwrap();
        let echo = which("echo").unwrap();
        let cmd = Cmd::new(echo, dir.path(), Duration::from_secs(10)).arg("$(touch pwned); `id`");
        let mut lines = Vec::new();
        run(&cmd, &CancellationToken::new(), |l| {
            lines.push(l.to_owned())
        })
        .await
        .unwrap();
        assert_eq!(lines, ["$(touch pwned); `id`"]);
        assert!(!dir.path().join("pwned").exists());
    }

    #[tokio::test]
    async fn environment_is_cleared() {
        // SAFETY: test-only; no other thread reads this variable.
        unsafe { std::env::set_var("FILEBRARY_TEST_SECRET", "s3cret") };
        let dir = tempfile::tempdir().unwrap();
        let cmd = Cmd::new(sh(), dir.path(), Duration::from_secs(10))
            .args(["-c", "echo \"[$FILEBRARY_TEST_SECRET]\""]);
        let mut lines = Vec::new();
        run(&cmd, &CancellationToken::new(), |l| {
            lines.push(l.to_owned())
        })
        .await
        .unwrap();
        assert_eq!(lines, ["[]"]);
    }

    #[tokio::test]
    async fn failure_reports_stderr() {
        let dir = tempfile::tempdir().unwrap();
        let cmd = Cmd::new(sh(), dir.path(), Duration::from_secs(10))
            .args(["-c", "echo bad input >&2; exit 3"]);
        let err = run(&cmd, &CancellationToken::new(), |_| {})
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("status 3") && msg.contains("bad input"),
            "{msg}"
        );
    }

    #[tokio::test]
    async fn cancel_and_timeout_kill_the_group() {
        let dir = tempfile::tempdir().unwrap();
        // The background child would keep running if only `sh` were killed.
        let cmd = Cmd::new(sh(), dir.path(), Duration::from_secs(30))
            .args(["-c", "(sleep 2; touch late) & sleep 30"]);
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            c2.cancel();
        });
        let err = run(&cmd, &cancel, |_| {}).await.unwrap_err();
        assert!(matches!(err, JobError::Cancelled));

        let cmd = Cmd::new(sh(), dir.path(), Duration::from_millis(200)).args(["-c", "sleep 30"]);
        let err = run(&cmd, &CancellationToken::new(), |_| {})
            .await
            .unwrap_err();
        assert!(err.to_string().contains("longer than"));

        tokio::time::sleep(Duration::from_millis(2500)).await;
        assert!(
            !dir.path().join("late").exists(),
            "background child survived"
        );
    }

    #[test]
    fn which_finds_programs() {
        assert!(which("sh").is_some());
        assert!(which("definitely-not-a-program-xyz").is_none());
        assert!(which("/definitely/not/here").is_none());
    }
}
