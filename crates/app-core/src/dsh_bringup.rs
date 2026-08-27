//! Kodex-managed `dsh web` bring-up.
//!
//! When a session selects the DeepSeek Harness agent, Kodex owns the whole
//! process lifecycle: writes `~/.kodex/dsh/settings.yaml` from the BYOK
//! provider catalog, spawns `dsh web --port 0`, discovers the bound loopback
//! endpoint from the readiness line, and registers the endpoint for the
//! session's `SessionConfig.harness_endpoint`.
//!
//! One `DshBringup` (process-wide, initialized by the desktop shell) shares a
//! single [`HarnessHostRegistry`] with the ACP harness backend, so a second
//! session targeting the same spawned endpoint reuses the same `dsh web`
//! process (one process, one SSE pair). The spawned child is attached to the
//! registry's `HarnessHost`; when the last sharing session exits, the host
//! teardown kills the child.

use crate::AppPaths;
use crate::settings::{build_dsh_settings_config, dsh_provider_keys, ensure_dsh_proxy_routing};
use dsh_bridge::{
    DshChild, HarnessHost, HarnessHostRegistry, SpawnDshWebConfig, spawn_dsh_web, write_settings,
};
use std::sync::{Arc, Mutex, OnceLock};

/// Process-wide bring-up singleton. Initialized once by the desktop shell so
/// the harness backend and `Application` share the same host registry.
static BRINGUP: OnceLock<Arc<DshBringup>> = OnceLock::new();

/// Initialize the process-wide bring-up singleton. The registry is shared with
/// the ACP harness backend (`acp_core::set_harness_backend`), so a spawned
/// `dsh web` host is reused across sessions and workspaces.
pub fn init_dsh_bringup(registry: Arc<HarnessHostRegistry>) {
    let _ = BRINGUP.set(Arc::new(DshBringup::new(registry)));
}

/// Access the process-wide bring-up singleton, if initialized. Falls back to a
/// standalone instance when unset (e.g. tests) so harness sessions do not
/// panic in unit tests that never call `init_dsh_bringup`.
pub fn dsh_bringup() -> Arc<DshBringup> {
    BRINGUP
        .get()
        .cloned()
        .unwrap_or_else(|| Arc::new(DshBringup::standalone()))
}

/// Kodex-managed `dsh web` lifecycle: settings write, spawn, endpoint
/// discovery, and shared-host registration.
pub struct DshBringup {
    registry: Arc<HarnessHostRegistry>,
    /// A single-thread tokio runtime used to drive the async spawn + readiness
    /// wait. Created once so the first spawn's worker threads are not churned
    /// per session.
    runtime: tokio::runtime::Runtime,
    /// The currently-managed `dsh web` host, if any. Only one managed host is
    /// supported (one spawned process); sessions that share its endpoint reuse
    /// it via the registry.
    managed: Mutex<Option<ManagedHost>>,
}

struct ManagedHost {
    endpoint: String,
    host: Arc<HarnessHost>,
}

impl DshBringup {
    pub fn new(registry: Arc<HarnessHostRegistry>) -> Self {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build dsh bringup tokio runtime");
        Self {
            registry,
            runtime,
            managed: Mutex::new(None),
        }
    }

    pub fn standalone() -> Self {
        Self::new(Arc::new(HarnessHostRegistry::new()))
    }

    pub fn registry(&self) -> &Arc<HarnessHostRegistry> {
        &self.registry
    }

    /// Ensure a `dsh web` process is running and return its loopback endpoint.
    ///
    /// On first call: write `settings.yaml`, spawn `dsh web --port 0`, wait for
    /// the readiness line, attach the child to the shared host, and cache the
    /// endpoint. Subsequent calls reuse the cached endpoint as long as the host
    /// is still alive.
    pub fn ensure_harness_endpoint(&self, paths: &AppPaths) -> Result<String, String> {
        if let Some(managed) = self
            .managed
            .lock()
            .map_err(|_| "dsh bringup lock poisoned")?
            .as_ref()
            && self.registry.host_alive(&managed.endpoint)
        {
            return Ok(managed.endpoint.clone());
        }

        // 1. Write ~/.kodex/dsh/settings.yaml from the BYOK provider catalog.
        //    First ensure the local codex_api_proxy is running and knows every
        //    configured source provider, so the harness's chat/completions
        //    traffic (routed through the proxy) can reach each upstream.
        ensure_dsh_proxy_routing(paths);
        let config = build_dsh_settings_config(paths).map_err(|e| e.to_string())?;
        write_settings(&paths.dsh_settings_path(), &config)
            .map_err(|e| format!("failed to write dsh settings.yaml: {e}"))?;

        // 2. Spawn `dsh web --port 0` and wait for the readiness line.
        let provider_keys = dsh_provider_keys(paths);
        let spawn_config = SpawnDshWebConfig {
            dsh_home: paths.dsh_dir().display().to_string(),
            provider_keys,
            extra_env: vec![
                ("DSH_TELEMETRY_DISABLED".to_string(), "1".to_string()),
                ("DSH_PERMISSION_MODE".to_string(), "danger-full-access".to_string()),
            ],
        };
        let (endpoint, child) = self
            .runtime
            .block_on(spawn_dsh_web(spawn_config))
            .map_err(|e| e.to_string())?;

        // 3. Acquire (or reuse) the shared host for this endpoint and attach the
        //    child so the last session exit tears the process down.
        let host = self
            .registry
            .acquire(endpoint.clone())
            .map_err(|e| e.to_string())?;
        host.attach_child(child);

        // 4. Cache the managed host so subsequent sessions reuse it.
        *self
            .managed
            .lock()
            .map_err(|_| "dsh bringup lock poisoned")? = Some(ManagedHost {
            endpoint: endpoint.clone(),
            host,
        });
        Ok(endpoint)
    }

    /// Shut down the managed `dsh web` process, if any. Called on app exit;
    /// the harness host teardown kills the spawned child.
    pub fn shutdown(&self) {
        if let Ok(mut managed) = self.managed.lock() {
            if let Some(host) = managed.take() {
                host.host.teardown();
            }
        }
    }
}

// Silence unused import when DshChild is only used via attach_child in host.rs.
#[allow(unused_imports)]
use dsh_bridge::DshChild as _DshChildRef;
