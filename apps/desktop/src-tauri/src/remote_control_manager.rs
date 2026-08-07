//! Owner of the mobile remote-control plane's shell-side state: device
//! identity, the current pairing code/QR, connection + subscription status,
//! and the kill switch. The long-lived relay-client driver task (dial,
//! auth, route, reconnect) is started separately; this manager is the
//! shared state it and the Tauri commands both touch.
//!
//! Fail-open: when disabled or disconnected, local sessions are entirely
//! unaffected — this manager never blocks the local command bridge.

use crate::commands::remote_control::RemoteControlStatus;
use relay_client::{
    AccountSession, DeviceIdentity, LoginClient, PairingCode, DEFAULT_PAIRING_TTL,
    auth_base_url_from_ws_endpoint, build_qr_payload,
};
use std::sync::Mutex;

pub struct RemoteControlManager {
    inner: Mutex<Inner>,
    app_paths: app_core::AppPaths,
    login: LoginClient,
}

struct Inner {
    enabled: bool,
    connected: bool,
    device_id: Option<String>,
    pairing_code: Option<PairingCode>,
    pairing_qr: Option<String>,
    subscription_active: Option<bool>,
    bound: bool,
    relay_endpoint: String,
    account_session: Option<AccountSession>,
    insecure_tls: bool,
}

impl RemoteControlManager {
    pub fn new(app_paths: app_core::AppPaths) -> Self {
        let enabled = std::env::var("KODEX_REMOTE_CONTROL")
            .map(|v| !matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "off"))
            .unwrap_or(true);
        let relay_endpoint = std::env::var("KODEX_RELAY_ENDPOINT")
            .unwrap_or_else(|_| "wss://120.48.49.190".to_string());
        // Skip TLS certificate verification for the relay endpoint. Hardcoded
        // on for the self-signed relay host (no domain yet); flip to false
        // and use wss://relay.kodex.app once a real cert exists.
        let insecure_tls = std::env::var("KODEX_RELAY_INSECURE_TLS")
            .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "on"))
            .unwrap_or(true);
        // Auth HTTP origin for the passwordless login endpoints. Prefer an
        // explicit override (dev relay on a separate port); otherwise derive
        // it from the WebSocket endpoint (prod reverse-proxies `/auth/*` on
        // the same origin). Empty when neither is configured — login
        // commands then fail fast with a clear message instead of dialing
        // nowhere.
        let auth_base = std::env::var("KODEX_RELAY_AUTH_ENDPOINT")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| auth_base_url_from_ws_endpoint(&relay_endpoint))
            .unwrap_or_default();
        let login = LoginClient::new(auth_base, insecure_tls);
        Self {
            inner: Mutex::new(Inner {
                enabled,
                connected: false,
                device_id: None,
                pairing_code: None,
                pairing_qr: None,
                subscription_active: None,
                bound: false,
                relay_endpoint,
                insecure_tls,
                account_session: None,
            }),
            app_paths,
            login,
        }
    }

    /// Load (or create) the device identity and record its id. Called once
    /// at app setup so `status()` can show the device id without touching
    /// the filesystem on every call.
    pub fn ensure_device_identity(&self) -> anyhow::Result<()> {
        let key_path = self.app_paths.root().join("remote-control-device.key");
        let identity = DeviceIdentity::load_or_create(&key_path)?;
        let device_id = identity.device_id();
        // Best-effort: load a previously stored account session so the UI
        // can surface logged-in state (and a later bind can reuse the
        // auth_token) without re-prompting for a login code.
        let account = AccountSession::load(&self.account_session_path()).unwrap_or(None);
        let mut inner = self.inner.lock().expect("rc manager mutex poisoned");
        inner.device_id = Some(device_id);
        inner.account_session = account;
        Ok(())
    }

    /// Path to the persisted account session JSON
    /// (`~/.kodex/remote-control-account.json`), next to the device key.
    /// Holds the email-OTP-acquired `auth_token` that feeds a subsequent
    /// `BindDeviceRequest`. Neither the E2E session key nor the device
    /// private key is stored here.
    fn account_session_path(&self) -> std::path::PathBuf {
        self.app_paths.root().join("remote-control-account.json")
    }

    /// `POST /auth/send-code { email }` on the relay's auth HTTP origin.
    /// Surfaces the server's rate-limit / validation messages verbatim.
    pub async fn send_login_code(&self, email: &str) -> Result<(), String> {
        self.login.send_code(email).await.map_err(|e| e.to_string())
    }

    /// `POST /auth/login { email, code }`, then persist the issued account
    /// session locally so it survives restarts (a later bind reuses the
    /// `auth_token`). The HTTP call runs without holding the manager mutex.
    pub async fn login_with_code(&self, email: &str, code: &str) -> Result<(), String> {
        let session = self
            .login
            .login(email, code)
            .await
            .map_err(|e| e.to_string())?;
        let path = self.account_session_path();
        session.persist(&path).map_err(|e| e.to_string())?;
        let mut inner = self.inner.lock().expect("rc manager mutex poisoned");
        inner.account_session = Some(session);
        Ok(())
    }

    /// Forget the stored account session (logout). Does not touch the
    /// device key or any in-flight pairing/binding.
    pub fn logout(&self) -> Result<(), String> {
        AccountSession::clear(&self.account_session_path()).map_err(|e| e.to_string())?;
        let mut inner = self.inner.lock().expect("rc manager mutex poisoned");
        inner.account_session = None;
        Ok(())
    }

    pub fn set_enabled(&self, enabled: bool) {
        let mut inner = self.inner.lock().expect("rc manager mutex poisoned");
        inner.enabled = enabled;
        if !enabled {
            inner.connected = false;
        }
    }

    /// Mint a fresh one-time pairing code + QR payload. Invalidates any
    /// previous code. Returns the QR JSON for the frontend, or None when
    /// the plane is disabled.
    pub fn mint_pairing_qr(&self) -> Result<Option<String>, String> {
        let mut inner = self.inner.lock().expect("rc manager mutex poisoned");
        if !inner.enabled {
            return Ok(None);
        }
        let key_path = self.app_paths.root().join("remote-control-device.key");
        let identity = DeviceIdentity::load_or_create(&key_path)
            .map_err(|e| format!("load device identity: {e}"))?;
        let code = PairingCode::mint(DEFAULT_PAIRING_TTL);
        let payload = build_qr_payload(&inner.relay_endpoint, &code, &identity.public_b64());
        let json = payload
            .to_json()
            .map_err(|e| format!("encode qr payload: {e}"))?;
        inner.pairing_code = Some(code);
        inner.pairing_qr = Some(json.clone());
        Ok(Some(json))
    }

    pub fn set_connected(&self, connected: bool) {
        let mut inner = self.inner.lock().expect("rc manager mutex poisoned");
        inner.connected = inner.enabled && connected;
    }

    pub fn set_subscription_active(&self, active: bool) {
        let mut inner = self.inner.lock().expect("rc manager mutex poisoned");
        inner.subscription_active = Some(active);
    }

    pub fn set_bound(&self, bound: bool) {
        let mut inner = self.inner.lock().expect("rc manager mutex poisoned");
        inner.bound = bound;
    }

    pub fn status(&self) -> RemoteControlStatus {
        let inner = self.inner.lock().expect("rc manager mutex poisoned");
        RemoteControlStatus {
            enabled: inner.enabled,
            connected: inner.connected,
            device_id: inner.device_id.clone(),
            pairing_qr: inner.pairing_qr.clone(),
            subscription_active: inner.subscription_active,
            bound: inner.bound,
            account_email: inner.account_session.as_ref().map(|s| s.email.clone()),
            logged_in: inner.account_session.is_some(),
        }
    }

    /// Whether TLS certificate verification is skipped for this relay
    /// endpoint (development against a self-signed host). Driven by the
    /// `KODEX_RELAY_INSECURE_TLS` env var; defaults to off.
    pub fn insecure_tls(&self) -> bool {
        self.inner.lock().expect("rc manager mutex poisoned").insecure_tls
    }
}
