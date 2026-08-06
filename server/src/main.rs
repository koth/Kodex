use maju_relay_server::{
    config::Config, db::Db, errors::Result, health, http_auth, state::AppState, subscription,
    transport,
};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
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
