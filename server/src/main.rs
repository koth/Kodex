use maju_relay_server::{
    config::Config, db::Db, errors::Result, health, http_auth, state::AppState, subscription,
    transport,
};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();

    // `maju-relay-server migrate` runs schema migrations and exits without
    // binding any sockets. Deploy runs this manually before restarting
    // systemd so a failed/unsafe migration is visible as a deploy failure
    // rather than surfacing after the service has already been restarted.
    // SQLite migrations are idempotent and load in the same order as the
    // server's normal startup (`Db::open`).
    if std::env::args().nth(1).as_deref() == Some("migrate") {
        let config = Config::from_env();
        tracing::info!(db_path = %config.db_path, "running schema migrations");
        let _db = Db::open(&config.db_path)?;
        tracing::info!("migrations complete");
        return Ok(());
    }

    let config = Config::from_env();
    tracing::info!(?config, "starting maju-relay-server");
    let db = Db::open(&config.db_path)?;
    let health_addr = config.health_addr;
    let state = AppState::new(config, db);

    // Background tasks: periodic subscription-expiry sweeper + health probe.
    tokio::spawn(subscription::run_sweeper(state.clone()));
    tokio::spawn(health::run(health_addr));
    // Passwordless email-OTP login surface (HTTP). Only started when a mail
    // provider is configured; without it the login flow cannot deliver codes.
    if !state.config.resend_api_key.is_empty() && !state.config.resend_from.is_empty() {
        tokio::spawn(http_auth::run(state.clone(), state.config.auth_http_addr));
    } else {
        tracing::warn!(
            "RESEND_API_KEY / RELAY_MAIL_FROM not set; passwordless login disabled"
        );
    }

    tokio::select! {
        res = transport::run(state) => {
            if let Err(e) = res {
                tracing::error!(error = %e, "server exited with error");
                return Err(e);
            }
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("ctrl-c received; shutting down");
        }
    }
    Ok(())
}
