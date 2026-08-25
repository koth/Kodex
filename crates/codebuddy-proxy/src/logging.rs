//! Diagnostic logging for the CodeBuddy proxy.
//!
//! All proxy diagnostics are routed through `tracing::debug!` with the
//! `codebuddy_proxy` target. The verbosity is controlled entirely by the
//! tracing subscriber's `EnvFilter` (e.g. `RUST_LOG=codebuddy_proxy=debug`),
//! so the old runtime `DEBUG_ENABLED` gate and the per-process
//! `~/.kodex/logs/codebuddy-proxy.log` file are gone.
//!
//! [`set_debug_enabled`] is kept as a no-op for API compatibility with the
//! desktop launcher, which still passes a `debug` flag in `ProxyConfig`.

/// No-op. Verbosity is now controlled by the tracing subscriber's filter
/// (e.g. `RUST_LOG=codebuddy_proxy=debug`), not a runtime flag.
pub fn set_debug_enabled(_enabled: bool) {}

/// Emit a single diagnostic line through `tracing::debug!`.
///
/// Callers still pass a pre-formatted string (legacy `&format!(...)` pattern).
/// The message is forwarded as-is to the `codebuddy_proxy` tracing target,
/// where the subscriber's filter decides whether it is emitted.
pub fn append_codebuddy_proxy_log(line: &str) {
    tracing::debug!(target: "codebuddy_proxy", "{}", line);
}
