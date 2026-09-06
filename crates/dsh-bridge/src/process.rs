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
        {
            match WindowsKillOnDropJob::for_child(child) {
                Ok(job) => self.kill_on_drop_job = Some(job),
                // A failure here (common when Kodex is already inside a job that
                // disallows nesting) silently removes the only crash-path safety
                // net: a shim's orphaned `node` then survives app exit. Log it so
                // the port-kill fallback in `HarnessHost::teardown` is the known
                // last line of defense rather than an invisible gap.
                Err(err) => tracing::warn!(
                    target: "dsh-bridge::spawn",
                    error = %err,
                    "kill-on-close job not attached; dsh tree relies on port-kill fallback",
                ),
            }
        }
    }

    /// No-op off Windows: job objects are a Windows-only mechanism; teardown
    /// on Unix kills the child directly via [`kill_child`].
    #[cfg(not(windows))]
    pub fn enable_kill_on_drop_job(&mut self) {}
}

/// Kill a spawned child: first close stdin (grace), then kill. Idempotent.
pub fn kill_child(child: DshChild) -> std::io::Result<()> {
    kill_child_reaped(child).map(|_| ())
}

/// Kill a spawned child and report whether the direct child was reaped;
/// callers may use `false` to decide whether to run the slower port-owner
/// fallback.
pub fn kill_child_reaped(child: DshChild) -> std::io::Result<bool> {
    #[cfg(windows)]
    let kill_on_drop_job = child.kill_on_drop_job;
    let mut reaped = false;
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
                    Ok(Some(_)) => {
                        reaped = true;
                        break;
                    }
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
    Ok(reaped)
}

/// Reap whatever process still holds the harness loopback port. This is the
/// last-resort fallback used after [`kill_child`] when the kill-on-close job
/// could not be attached and the `dsh` shim (`cmd.exe`/volta) has already
/// exited, leaving the real `node` server orphaned with no parent handle to
/// kill. No-op when the direct kill already freed the port.
///
/// Implemented via `netstat` + `taskkill` (no unsafe FFI). The exact
/// `127.0.0.1:<port>` local-address token is matched to avoid terminating an
/// unrelated process that happens to share a port prefix.
#[cfg(windows)]
pub(crate) fn kill_port_owner(port: u16) {
    let out = match std::process::Command::new("netstat")
        .args(["-ano", "-p", "tcp"])
        .output()
    {
        Ok(out) => out,
        Err(err) => {
            tracing::debug!(
                target: "dsh-bridge::spawn",
                error = %err,
                "netstat failed for port-kill"
            );
            return;
        }
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let local = format!("127.0.0.1:{port}");
    // netstat lists both LISTENING and ESTABLISHED rows for the same port/PID;
    // kill each owning PID at most once.
    let mut killed = std::collections::HashSet::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let proto = it.next();
        let Some(laddr) = it.next() else {
            continue;
        };
        if proto != Some("TCP") || laddr != local {
            continue;
        }
        // `netstat -ano` lists the owning PID as the last whitespace token.
        let Some(pid_token) = it.last() else { continue };
        let Ok(pid) = pid_token.parse::<u32>() else {
            continue;
        };
        if pid == 0 || killed.contains(&pid) {
            continue;
        }
        match std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID"])
            .arg(pid.to_string())
            .output()
        {
            Ok(o) if o.status.success() => {
                killed.insert(pid);
                tracing::info!(
                    target: "dsh-bridge::spawn",
                    port,
                    pid,
                    "killed orphan owning harness port"
                );
            }
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                tracing::debug!(
                    target: "dsh-bridge::spawn",
                    port,
                    pid,
                    stderr = stderr.trim(),
                    "taskkill non-success for port owner"
                );
            }
            Err(err) => {
                tracing::debug!(
                    target: "dsh-bridge::spawn",
                    port,
                    pid,
                    error = %err,
                    "taskkill failed for port owner"
                );
            }
        }
    }
}

#[cfg(not(windows))]
pub(crate) fn kill_port_owner(_port: u16) {}

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

    // On Windows, bypass the shim when possible. `dsh` on PATH is usually a
    // `.cmd` shim (`npm` or `volta`); launching it via `cmd.exe /C` with
    // `CREATE_NO_WINDOW` hides cmd.exe but the shim then re-launches a console
    // grandchild (`volta.exe`/`node.exe`). That grandchild has no parent console
    // (its parent was created with `CREATE_NO_WINDOW`), so Windows allocates a
    // fresh visible console — the "window that flashes by". Resolving the real
    // `node <dsh entry>` and spawning node directly keeps the console-subsystem
    // process itself under `CREATE_NO_WINDOW` (no grandchild, no flash).
    #[cfg(windows)]
    if let Some((node, entry)) = resolve_dsh_direct_command(&dsh) {
        tracing::info!(
            target: "dsh-bridge::spawn",
            node = %node.display(),
            entry = %entry.display(),
            "launching dsh web via node directly (bypassing shim)",
        );
        let mut cmd = Command::new(node);
        cmd.arg(entry);
        return spawn_with_args(cmd, &dsh, &["web", "--port", "0", "--no-open"], config).await;
    }

    // `--port 0` lets the OS pick a free loopback port; the readiness line
    // reports the actual bound port.
    //
    // The `dsh` CLI found on PATH is often a shim script: `dsh.cmd` (Windows
    // npm/volta shim → `cmd.exe /C`) or a `#!/bin/sh` wrapper (Unix). A raw
    // `Command::new(path)` cannot execute a batch script directly on Windows
    // and ignores shebangs on Unix, so wrap the same way kodex's ACP spawn
    // does (`agent_spawn_command`).
    let mut cmd = if cfg!(windows) && is_windows_batch_script(&dsh) {
        tracing::warn!(
            target: "dsh-bridge::spawn",
            dsh = %dsh.display(),
            "falling back to shim launch (direct node resolution unavailable)",
        );
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
    // `--no-open` suppresses the default-browser handoff. dsh 0.1.2 prints
    // an authenticated readiness URL containing the one-time launch token;
    // HttpClient exchanges that token for its shared auth cookie.
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

/// Resolve the real `node <dsh entry>` launch for the installed `dsh` CLI on
/// this machine, bypassing the npm/volta shim. Returns `(node, entry)` when
/// resolvable. Used by callers that must run a `dsh` subcommand (e.g. a version
/// check) without letting the shim chain (`cmd.exe → volta.exe → node`) open a
/// visible console window.
pub fn resolve_dsh_launch() -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    let dsh = find_dsh_binary()?;
    #[cfg(windows)]
    {
        if let Some(direct) = resolve_dsh_direct_command(&dsh) {
            return Some(direct);
        }
    }
    None
}

/// Resolve the real `node <npm-cli.js>` launch behind the npm shim, so the npm
/// registry query can run without letting the npm/Volta shim chain open a
/// visible console window on Windows.
#[cfg(windows)]
pub fn resolve_npm_launch() -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    let npm = search_paths()
        .iter()
        .map(|dir| dir.join("npm.cmd"))
        .find(|candidate| candidate.is_file())?;
    let text = std::fs::read_to_string(&npm).ok()?;

    let node = if is_volta_npm_shim(&text, &npm) {
        find_volta_node().or_else(|| find_binary("node"))
    } else {
        find_binary("node").or_else(find_volta_node)
    }?;

    // npm-style shim: extract the `node_modules/.../npm-cli.js` path from the
    // shim text, if present.
    if let Some(entry) = parse_npm_shim_entry(&text, &npm) {
        return Some((node, entry));
    }

    // Volta's npm launcher is a tiny `npm.exe` (not a batch file with an npm
    // JS path in it). Locate the npm installation that Volta manages.
    if is_volta_npm_shim(&text, &npm) {
        let entry = resolve_volta_npm_entry()?;
        return Some((node, entry));
    }

    None
}

/// Whether `npm.cmd` is a Volta launcher shim rather than the standard npm shim.
#[cfg(windows)]
fn is_volta_npm_shim(text: &str, shim: &std::path::Path) -> bool {
    text.contains("volta")
        || text.contains("%~dpn0.exe")
        || shim.to_string_lossy().contains("Volta")
}

/// Locate the npm CLI entry managed by Volta under
/// `%LOCALAPPDATA%\Volta\tools\image\npm\<version>\bin\npm-cli.js`.
#[cfg(windows)]
fn resolve_volta_npm_entry() -> Option<std::path::PathBuf> {
    let mut roots: Vec<std::path::PathBuf> = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        roots.push(
            std::path::PathBuf::from(local)
                .join("Volta")
                .join("tools")
                .join("image")
                .join("npm"),
        );
    }
    if let Some(program_files) = std::env::var_os("ProgramFiles") {
        roots.push(
            std::path::PathBuf::from(program_files)
                .join("Volta")
                .join("tools")
                .join("image")
                .join("npm"),
        );
    }
    for npm_root in roots {
        let Ok(rd) = std::fs::read_dir(&npm_root) else {
            continue;
        };
        let mut versions: Vec<std::path::PathBuf> = rd
            .flatten()
            .map(|entry| entry.path())
            .filter(|p| p.is_dir())
            .collect();
        versions.sort();
        for version in versions.into_iter().rev() {
            let entry = version.join("bin").join("npm-cli.js");
            if entry.is_file() {
                return Some(entry);
            }
        }
    }
    None
}

/// No-op on non-Windows; npm on Unix can be spawned directly without a
/// console-window shim.
#[cfg(not(windows))]
pub fn resolve_npm_launch() -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    None
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

/// Resolve the real `node <dsh entry>` invocation behind a Windows shim, so the
/// console-subsystem `node` can be spawned directly under `CREATE_NO_WINDOW`
/// instead of through the shim chain (which lets `volta.exe`/`node.exe` allocate
/// a fresh visible console). Returns `None` when the shim cannot be resolved —
/// the caller then falls back to the shim path.
///
/// Handles the two common npm/volta shim shapes:
/// - npm `.cmd`: the entry `.js` path is spelled out in the shim text.
/// - volta `.cmd`: `volta run <name>`, resolved via Volta's package layout.
#[cfg(windows)]
fn resolve_dsh_direct_command(
    dsh: &std::path::Path,
) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    if !is_windows_batch_script(dsh) {
        return None;
    }
    let text = std::fs::read_to_string(dsh).ok()?;
    // For a volta shim, prefer Volta's real node image over `node` on PATH:
    // `C:\Program Files\Volta\node.exe` is itself a small shim that re-launches
    // the real node, which would allocate a fresh visible console (the flash)
    // just like the dsh shim does. The real node lives under
    // `%LOCALAPPDATA%\Volta\tools\image\node\<ver>\node.exe`.
    let node = if text.contains("volta") {
        find_volta_node().or_else(|| find_binary("node"))
    } else {
        find_binary("node").or_else(find_volta_node)
    }?;

    // npm-style shim: extract the `node_modules/.../bin.js` path from the text.
    if let Some(entry) = parse_npm_shim_entry(&text, dsh) {
        return Some((node, entry));
    }

    // volta-style shim: `volta run dsh ...` — locate the installed package by
    // its bin name under Volta's image package layout.
    if text.contains("volta") {
        let name = dsh.file_stem()?.to_str()?.to_string();
        let entry = resolve_volta_package_entry(&name)?;
        return Some((node, entry));
    }

    None
}

/// Extract the `node_modules/<pkg>/<entry>.js` path an npm `.cmd` shim invokes.
/// The shim text uses `%~dp0` for the shim's directory, which we expand.
#[cfg(windows)]
fn parse_npm_shim_entry(text: &str, shim: &std::path::Path) -> Option<std::path::PathBuf> {
    let dir = shim.parent()?;
    let parts: Vec<&str> = text.split('"').collect();
    // After split on `"`, odd indices are the quoted token contents.
    for token in parts.iter().skip(1).step_by(2) {
        if !token.ends_with(".js") || !token.contains("node_modules") {
            continue;
        }
        let expanded = token.replace("%~dp0", dir.to_string_lossy().as_ref());
        let candidate = std::path::PathBuf::from(expanded.trim());
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Resolve a Volta-installed global package's entry file for the given shim
/// name. Volta stores packages under `%LOCALAPPDATA%\Volta\tools\image\packages`
/// in two shapes:
/// - unscoped: `packages\<name>\node_modules\<name>\package.json`
/// - scoped:   `packages\<scope>\<name>\node_modules\<scope>\<name>\package.json`
/// We only probe those two exact slots — never descending into dependency
/// `node_modules` — then read `package.json`'s `bin.<name>` entry.
#[cfg(windows)]
fn resolve_volta_package_entry(name: &str) -> Option<std::path::PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    let root = std::path::PathBuf::from(local)
        .join("Volta")
        .join("tools")
        .join("image")
        .join("packages");

    // Unscoped package: packages\<name>\node_modules\<name>\package.json
    let unscoped = root
        .join(name)
        .join("node_modules")
        .join(name)
        .join("package.json");
    if let Some(entry) = bin_entry_from_package_json(&unscoped, name) {
        return Some(entry);
    }

    // Scoped package: iterate `@scope` directories under `packages`.
    let rd = std::fs::read_dir(&root).ok()?;
    for scope_entry in rd.flatten() {
        let scope = scope_entry.path();
        let is_scope = scope
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('@'));
        if !scope.is_dir() || !is_scope {
            continue;
        }
        let pkg_json = scope
            .join(name)
            .join("node_modules")
            .join(scope.file_name()?)
            .join(name)
            .join("package.json");
        if let Some(entry) = bin_entry_from_package_json(&pkg_json, name) {
            return Some(entry);
        }
    }
    None
}

/// Read `package.json` and return the absolute path of the `bin.<name>` entry.
#[cfg(windows)]
fn bin_entry_from_package_json(
    pkg_json: &std::path::Path,
    name: &str,
) -> Option<std::path::PathBuf> {
    let raw = std::fs::read_to_string(pkg_json).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let entry = match value.get("bin")? {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(map) => map.get(name)?.as_str()?.to_string(),
        _ => return None,
    };
    let candidate = pkg_json.parent()?.join(entry.as_str());
    candidate.is_file().then_some(candidate)
}

/// Locate Volta's real `node.exe` under `%LOCALAPPDATA%\Volta\tools\image\node`
/// (falling back to `%ProgramFiles%\Volta\tools\image\node`). This is the actual
/// node binary, not the `C:\Program Files\Volta\node.exe` shim that re-launches
/// it. Volta names version dirs like `22.22.1`; the newest is preferred.
#[cfg(windows)]
fn find_volta_node() -> Option<std::path::PathBuf> {
    let mut roots: Vec<std::path::PathBuf> = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        roots.push(
            std::path::PathBuf::from(local)
                .join("Volta")
                .join("tools")
                .join("image")
                .join("node"),
        );
    }
    if let Some(program_files) = std::env::var_os("ProgramFiles") {
        roots.push(
            std::path::PathBuf::from(program_files)
                .join("Volta")
                .join("tools")
                .join("image")
                .join("node"),
        );
    }
    for node_root in roots {
        let Ok(rd) = std::fs::read_dir(&node_root) else {
            continue;
        };
        let mut versions: Vec<std::path::PathBuf> = rd
            .flatten()
            .map(|entry| entry.path())
            .filter(|p| p.is_dir())
            .collect();
        versions.sort();
        for v in versions.into_iter().rev() {
            let node = v.join("node.exe");
            if node.is_file() {
                return Some(node);
            }
        }
    }
    None
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
