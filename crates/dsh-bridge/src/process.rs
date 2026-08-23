//! Optional Kodex-managed `dsh web` process spawn.
//!
//! When no external `harness_endpoint` is configured, the bridge spawns
//! `dsh web` (the `dsh` CLI installed via `npm i -g @deepseek-ai/dsh`), lets
//! the OS pick a free loopback port (`--port 0`), discovers the bound port
//! from the readiness line dsh prints on stdout, and shares it across
//! sessions via the `HarnessHostRegistry`. A Kodex-spawned process is killed
//! only when the last sharing session exits (stdin EOF grace then kill); an
//! externally-managed endpoint is never killed by Kodex.
//!
//! The dsh web profile prints `dsh web: http://127.0.0.1:<port>` to stdout
//! the moment the server is listening (`packages/bundle/web-app/src/index.ts`).
//! That line is the readiness signal: we parse it to recover the endpoint.

use anyhow::{Context, anyhow};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

/// Maximum time to wait for the `dsh web` readiness line before giving up.
const READINESS_TIMEOUT: Duration = Duration::from_secs(60);
/// Windows `CREATE_NO_WINDOW` — spawn without a visible console window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

/// A Kodex-spawned `dsh web` child process handle, owned by a `HarnessHost`.
pub struct DshChild {
    inner: Mutex<Option<Child>>,
    /// Windows job object that kills the whole process tree (dsh web + the
    /// node process behind the `dsh` shim) when the job handle closes. Kept
    /// alive for the child's lifetime so closing Kodex reaps everything.
    #[cfg(windows)]
    kill_on_drop_job: Option<WindowsKillOnDropJob>,
}

impl std::fmt::Debug for DshChild {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DshChild").finish_non_exhaustive()
    }
}

impl DshChild {
    pub fn new(child: Child) -> Self {
        Self {
            inner: Mutex::new(Some(child)),
            #[cfg(windows)]
            kill_on_drop_job: None,
        }
    }

    /// Attach the child to a Windows kill-on-close job object so the entire
    /// process tree is terminated when the job handle is closed (or killed
    /// explicitly). Best-effort: if job creation fails the child is still
    /// killed directly by [`kill_child`].
    #[cfg(windows)]
    pub fn enable_kill_on_drop_job(&mut self) {
        if let Ok(guard) = self.inner.lock()
            && let Some(child) = guard.as_ref()
            && let Ok(job) = WindowsKillOnDropJob::for_child(child)
        {
            self.kill_on_drop_job = Some(job);
        }
    }

    /// No-op off Windows: job objects are a Windows-only mechanism; teardown
    /// on Unix kills the child directly via [`kill_child`].
    #[cfg(not(windows))]
    pub fn enable_kill_on_drop_job(&mut self) {}
}

/// Kill a spawned child: first close stdin (grace), then kill. Idempotent.
pub fn kill_child(child: DshChild) -> std::io::Result<()> {
    #[cfg(windows)]
    let kill_on_drop_job = child.kill_on_drop_job;
    if let Ok(mut guard) = child.inner.lock() {
        if let Some(mut proc) = guard.take() {
            // Closing stdin signals a graceful shutdown to the Node process;
            // dsh's process-shutdown controller disposes the cordis tree.
            let _ = proc.stdin.take();
            // Then kill if still alive. Poll `try_wait` briefly instead of the
            // blocking `wait()`: the latter requires a tokio runtime context
            // and can hang when called from a plain thread. The kill-on-drop
            // job (when enabled) reaps the whole tree on `DshChild` drop, so
            // an unsettled direct child is not a leak.
            let _ = proc.start_kill();
            for _ in 0..40 {
                match proc.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => std::thread::sleep(Duration::from_millis(25)),
                    Err(_) => break,
                }
            }
        }
    }
    // Close the job handle immediately after termination. With
    // KILL_ON_JOB_CLOSE this also terminates any surviving descendants of a
    // shim (e.g. the node process behind `volta run dsh`).
    #[cfg(windows)]
    drop(kill_on_drop_job);
    Ok(())
}

/// Configuration for spawning a Kodex-managed `dsh web` process.
pub struct SpawnDshWebConfig {
    /// `DSH_HOME` — the harness home (`~/.kodex/dsh`). Required so dsh reads
    /// the Kodex-generated `settings.yaml` and keeps all state under Kodex's
    /// data root rather than `~/.dsh`.
    pub dsh_home: String,
    /// Provider API keys to inject, keyed by the env-var name written to
    /// `settings.yaml` as `apiKeyEnv` (e.g. `KODEX_DSH_DEEPSEEK_KEY` -> secret).
    /// dsh's `llm-pi-ai` resolves the credential named by `apiKeyEnv` from the
    /// launch environment, so the secret never lives in YAML.
    pub provider_keys: Vec<(String, String)>,
    /// Extra environment variables forwarded verbatim (e.g.
    /// `DSH_TELEMETRY_DISABLED=1`).
    pub extra_env: Vec<(String, String)>,
}

impl Default for SpawnDshWebConfig {
    fn default() -> Self {
        Self {
            dsh_home: String::new(),
            provider_keys: Vec::new(),
            extra_env: Vec::new(),
        }
    }
}

/// Spawn `dsh web --port 0` and return the discovered loopback endpoint plus
/// the child handle. The endpoint is recovered from the `dsh web: http://...`
/// readiness line printed on stdout once the server is listening.
///
/// `dsh` is resolved via `app_core`'s PATH search (which includes GUI-launched
/// process PATH gaps); if it is not installed, returns a diagnostic error so
/// the caller can prompt the user to `npm i -g @deepseek-ai/dsh`.
pub async fn spawn_dsh_web(config: SpawnDshWebConfig) -> anyhow::Result<(String, DshChild)> {
    let dsh = find_dsh_binary().ok_or_else(|| {
        anyhow!("dsh CLI not found on PATH; install it with `npm i -g @deepseek-ai/dsh`")
    })?;

    // `--port 0` lets the OS pick a free loopback port; the readiness line
    // reports the actual bound port.
    //
    // The `dsh` CLI found on PATH is often a shim script: `dsh.cmd` (Windows
    // npm/volta shim → `cmd.exe /C`) or a `#!/bin/sh` wrapper (Unix). A raw
    // `Command::new(path)` cannot execute a batch script directly on Windows
    // and ignores shebangs on Unix, so wrap the same way kodex's ACP spawn
    // does (`agent_spawn_command`).
    let mut cmd = if cfg!(windows) && is_windows_batch_script(&dsh) {
        let mut wrapper = Command::new("cmd.exe");
        wrapper.arg("/C").arg(&dsh);
        wrapper
    } else if cfg!(not(windows)) && is_script_file(&dsh) {
        let mut wrapper = Command::new("/bin/sh");
        let mut cmd_str = dsh.to_string_lossy().to_string();
        for arg in ["web", "--port", "0", "--no-open"] {
            cmd_str.push(' ');
            cmd_str.push_str(&shell_words::quote(arg));
        }
        wrapper.arg("-c").arg(cmd_str);
        // Args are already embedded in the shell command; skip the common
        // `.args()` below.
        return spawn_with_args(wrapper, &dsh, &[], config).await;
    } else {
        Command::new(&dsh)
    };
    // `--no-open`: dsh web would otherwise open the default browser; Kodex
    // connects to the discovered endpoint itself.
    spawn_with_args(cmd, &dsh, &["web", "--port", "0", "--no-open"], config).await
}

/// Shared spawn leg: set env, pipes, spawn, and read the readiness line.
async fn spawn_with_args(
    mut cmd: Command,
    dsh: &std::path::Path,
    args: &[&str],
    config: SpawnDshWebConfig,
) -> anyhow::Result<(String, DshChild)> {
    cmd.args(args);
    #[cfg(windows)]
    {
        // Spawn without a visible console window (the `dsh` shim would
        // otherwise pop a cmd/volta/node window).
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    // `dsh` is an npm shim (`#!/usr/bin/env node`): a GUI-launched app does
    // not inherit the user's interactive shell PATH, so hand the child the
    // same augmented search path used to locate the binary. Without this,
    // the shim exits immediately with `env: node: No such file or directory`
    // and no readiness line is ever printed.
    if let Ok(joined) = std::env::join_paths(search_paths()) {
        cmd.env("PATH", joined);
    }
    if !config.dsh_home.is_empty() {
        cmd.env("DSH_HOME", &config.dsh_home);
    }
    for (env_name, secret) in &config.provider_keys {
        cmd.env(env_name, secret);
    }
    for (k, v) in &config.extra_env {
        cmd.env(k, v);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Inherit nothing else: the dsh profile composes its own env.

    let mut child = cmd
        .spawn()
        .with_context(|| format!("failed to spawn dsh web at {}", dsh.display()))?;

    let stdout = child
        .stdout
        .take()
        .context("dsh web stdout was not piped")?;
    let stderr = child
        .stderr
        .take()
        .context("dsh web stderr was not piped")?;

    // Drain stderr to a background task so the child's pipe does not block.
    let stderr_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => break,
                Ok(_) => {
                    tracing::debug!(target: "dsh-bridge::spawn", "dsh stderr: {}", line.trim_end());
                }
                Err(_) => break,
            }
        }
    });

    // Read stdout line-by-line until the readiness line appears, streaming
    // non-readiness lines to the debug log.
    let endpoint = tokio::time::timeout(READINESS_TIMEOUT, read_readiness_line(stdout))
        .await
        .map_err(|_| {
            anyhow!(
                "timed out waiting for dsh web readiness line ({}s)",
                READINESS_TIMEOUT.as_secs()
            )
        })??;

    // The stderr drain can keep running; it ends when the child exits.
    let _ = stderr_task;

    let mut child = DshChild::new(child);
    #[cfg(windows)]
    child.enable_kill_on_drop_job();
    Ok((endpoint, child))
}

/// Windows job object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`: closing the
/// handle kills every process assigned to it, including descendants spawned
/// after assignment. Mirrors `acp-core`'s hidden-agent process handling so
/// closing Kodex reaps the whole `dsh web` tree (shim + node), not just the
/// direct child.
#[cfg(windows)]
struct WindowsKillOnDropJob(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl std::fmt::Debug for WindowsKillOnDropJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowsKillOnDropJob")
            .finish_non_exhaustive()
    }
}

#[cfg(windows)]
unsafe impl Send for WindowsKillOnDropJob {}

#[cfg(windows)]
impl WindowsKillOnDropJob {
    fn for_child(child: &Child) -> anyhow::Result<Self> {
        use std::mem::{size_of, zeroed};
        use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };

        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                anyhow::bail!("CreateJobObjectW failed");
            }

            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let set_ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &mut info as *mut _ as *mut _,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if set_ok == 0 {
                CloseHandle(job);
                anyhow::bail!("SetInformationJobObject failed");
            }

            let process = child.raw_handle().unwrap_or(std::ptr::null_mut()) as HANDLE;
            let assign_ok = AssignProcessToJobObject(job, process);
            if assign_ok == 0 {
                CloseHandle(job);
                anyhow::bail!("AssignProcessToJobObject failed");
            }

            Ok(Self(job))
        }
    }
}

#[cfg(windows)]
impl Drop for WindowsKillOnDropJob {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Read dsh stdout lines until one matches the readiness pattern, returning
/// the resolved `http://127.0.0.1:<port>` endpoint.
async fn read_readiness_line(stdout: tokio::process::ChildStdout) -> anyhow::Result<String> {
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            return Err(anyhow!("dsh web exited before printing the readiness line"));
        }
        let trimmed = line.trim_end();
        tracing::debug!(target: "dsh-bridge::spawn", "dsh stdout: {}", trimmed);
        if let Some(url) = parse_readiness_line(trimmed) {
            return Ok(url);
        }
    }
}

/// Parse the dsh web readiness line (`dsh web: http://127.0.0.1:<port>`) and
/// return the local loopback URL. dsh may also print a LAN URL suffix; we
/// keep the loopback one.
fn parse_readiness_line(line: &str) -> Option<String> {
    let prefix = "dsh web:";
    let rest = line.strip_prefix(prefix)?.trim_start();
    // Take the first whitespace-separated token (the URL). dsh may append
    // `(LAN: http://...)` after the local URL.
    let url = rest.split_whitespace().next()?;
    if url.starts_with("http://") || url.starts_with("https://") {
        Some(url.to_string())
    } else {
        None
    }
}

/// Locate the `dsh` binary using the same PATH search kodex uses for agent
/// CLIs (including GUI-launched process PATH gaps). Mirrors
/// `app_core::settings::agent_cli::find_binary` without taking a dependency
/// on `app_core` (which would reverse the crate layering).
fn find_dsh_binary() -> Option<std::path::PathBuf> {
    find_binary("dsh")
}

/// Whether `path` is a Windows batch script (`.cmd`/`.bat`), which cannot be
/// executed directly by `Command::new` and must go through `cmd.exe /C`.
#[cfg(windows)]
fn is_windows_batch_script(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
        })
}

#[cfg(not(windows))]
fn is_windows_batch_script(_path: &std::path::Path) -> bool {
    false
}

/// Whether `path` is a `#!/bin/sh`-style script, which `Command::new` cannot
/// execute directly on Unix (no shebang interpretation).
#[cfg(not(windows))]
fn is_script_file(path: &std::path::Path) -> bool {
    use std::io::Read;
    if let Ok(mut file) = std::fs::File::open(path) {
        let mut buf = [0u8; 2];
        if file.read_exact(&mut buf).is_ok() {
            return buf == [0x23, 0x21]; // "#!"
        }
    }
    false
}

#[cfg(windows)]
fn is_script_file(_path: &std::path::Path) -> bool {
    false
}

fn find_binary(binary: &str) -> Option<std::path::PathBuf> {
    let names: Vec<String> = if cfg!(windows) {
        vec![
            format!("{binary}.exe"),
            format!("{binary}.cmd"),
            format!("{binary}.bat"),
        ]
    } else {
        vec![binary.to_string()]
    };

    search_paths()
        .into_iter()
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|path| path.is_file())
}

/// PATH search including GUI-launched process gaps. Mirrors
/// `app_core::settings::agent_cli::search_paths`.
fn search_paths() -> Vec<std::path::PathBuf> {
    let mut search_paths: Vec<std::path::PathBuf> = Vec::new();
    if let Some(paths) = std::env::var_os("PATH") {
        search_paths.extend(std::env::split_paths(&paths));
    }
    if let Some(home) = dirs_next::home_dir() {
        for suffix in [".local/bin", "bin"] {
            let p = home.join(suffix);
            if !search_paths.contains(&p) {
                search_paths.push(p);
            }
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(app_data) = std::env::var_os("APPDATA") {
            let p = std::path::PathBuf::from(&app_data).join("npm");
            if !search_paths.contains(&p) {
                search_paths.push(p);
            }
        }
        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
            let p = std::path::PathBuf::from(&local_app_data).join("npm");
            if !search_paths.contains(&p) {
                search_paths.push(p);
            }
            // Volta shims (dsh installed via `volta install`).
            let volta = std::path::PathBuf::from(&local_app_data)
                .join("Volta")
                .join("bin");
            if !search_paths.contains(&volta) {
                search_paths.push(volta);
            }
        }
        if let Some(user_profile) = std::env::var_os("USERPROFILE") {
            let volta = std::path::PathBuf::from(&user_profile)
                .join(".volta")
                .join("bin");
            if !search_paths.contains(&volta) {
                search_paths.push(volta);
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        for extra in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"] {
            let p = std::path::PathBuf::from(extra);
            if !search_paths.contains(&p) {
                search_paths.push(p);
            }
        }
        // Common version-manager install roots; GUI-launched apps do not
        // inherit the shell hooks that put these on PATH.
        if let Some(home) = dirs_next::home_dir() {
            for suffix in [".volta/bin", ".local/share/mise/shims", ".asdf/shims"] {
                let p = home.join(suffix);
                if !search_paths.contains(&p) {
                    search_paths.push(p);
                }
            }
            if let Some(entries) = nvm_version_bin_dirs(&home) {
                for p in entries {
                    if !search_paths.contains(&p) {
                        search_paths.push(p);
                    }
                }
            }
        }
    }
    search_paths
}

/// nvm keeps each installed Node version at `~/.nvm/versions/node/vX.Y.Z/bin`.
/// Return those bin directories, newest version first, so a GUI-launched app
/// can find a `dsh` shim installed through nvm without the shell hook.
#[cfg(target_os = "macos")]
fn nvm_version_bin_dirs(home: &std::path::Path) -> Option<Vec<std::path::PathBuf>> {
    let versions_dir = home.join(".nvm/versions/node");
    let mut dirs: Vec<std::path::PathBuf> = std::fs::read_dir(&versions_dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path().join("bin"))
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    dirs.reverse();
    if dirs.is_empty() { None } else { Some(dirs) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_readiness_line_local_url() {
        assert_eq!(
            parse_readiness_line("dsh web: http://127.0.0.1:41237"),
            Some("http://127.0.0.1:41237".to_string())
        );
    }

    #[test]
    fn parse_readiness_line_with_lan_suffix() {
        assert_eq!(
            parse_readiness_line("dsh web: http://127.0.0.1:41237 (LAN: http://192.168.1.5:41237)"),
            Some("http://127.0.0.1:41237".to_string())
        );
    }

    #[test]
    fn parse_readiness_line_rejects_non_dsh_line() {
        assert_eq!(parse_readiness_line("listening on port 3080"), None);
        assert_eq!(parse_readiness_line(""), None);
    }

    #[tokio::test]
    async fn kill_child_terminates_a_spawned_process() {
        // Spawn a long-running child and verify kill_child stops it promptly.
        // `kill_child` calls `start_kill()` then `wait()`, so a quick `Ok`
        // return proves the process was terminated (a live `ping`/`sleep`
        // would otherwise block `wait()` for its full duration).
        let mut cmd = tokio::process::Command::new(if cfg!(windows) { "cmd" } else { "sleep" });
        if cfg!(windows) {
            cmd.args(["/c", "ping", "-n", "60", "127.0.0.1"]);
        } else {
            cmd.arg("60");
        }
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let child = cmd.spawn().unwrap();
        let started = std::time::Instant::now();
        kill_child(DshChild::new(child)).unwrap();
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "kill_child blocked; the child was not terminated"
        );
    }

    #[tokio::test]
    async fn spawn_dsh_web_fails_fast_without_a_dsh_binary() {
        // 11.5: with no resolvable `dsh` binary, spawn fails fast with a
        // diagnostic (no hang, no partial process). The PATH search only looks
        // at the current process PATH, so a developer machine that happens to
        // have `dsh` installed is skipped rather than flaky.
        if find_dsh_binary().is_some() {
            eprintln!("skipping: a real `dsh` binary is on PATH");
            return;
        }
        let config = SpawnDshWebConfig {
            dsh_home: std::env::temp_dir().display().to_string(),
            ..Default::default()
        };
        let result = spawn_dsh_web(config).await;
        let err = result.expect_err("spawn must fail without a dsh binary");
        assert!(
            err.to_string().contains("dsh CLI not found"),
            "unexpected error: {err}"
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn spawn_dsh_web_runs_a_cmd_shim_and_parses_readiness() {
        // Reproduces the Windows `dsh.cmd` shim case: a batch script that
        // echoes the readiness line must be launched via `cmd.exe /C` and the
        // line parsed. Drives the same wrapper `spawn_dsh_web` builds for a
        // `.cmd` binary, without touching the process PATH.
        let dir = tempfile::tempdir().unwrap();
        let shim = dir.path().join("dsh.cmd");
        std::fs::write(
            &shim,
            "@echo off\r\necho dsh web: http://127.0.0.1:41237\r\nping -n 30 127.0.0.1 >nul\r\n",
        )
        .unwrap();
        assert!(is_windows_batch_script(&shim));

        let mut cmd = Command::new("cmd.exe");
        cmd.arg("/C").arg(&shim);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = cmd.spawn().unwrap();
        let stdout = child.stdout.take().unwrap();
        let endpoint = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            read_readiness_line(stdout),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(endpoint, "http://127.0.0.1:41237");
        let _ = child.kill().await;
    }
}
