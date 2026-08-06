//! Outbound transactional mail for the passwordless login flow.
//!
//! `MailSender` abstracts delivery so the core login logic (`login.rs`) can
//! be unit-tested with an in-memory sender, and the production
//! `ResendSender` (https://api.resend.com) can be swapped for another
//! provider by implementing the trait. Resend is the recommended free-tier
//! provider; the sender is HTTP-only and holds no persistent connection.

use std::future::Future;
use std::pin::Pin;

use crate::errors::{RelayError, Result};

/// Send a transactional email. Returns a boxed future so the trait is
/// dyn-compatible (a native `async fn` in a trait is not object-safe). This
/// mirrors the relay server's "no new dependency" stance — `async-trait`
/// would work too but pulls a macro crate.
pub trait MailSender: Send + Sync {
    fn send<'a>(
        &'a self,
        to: &'a str,
        subject: &'a str,
        body: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;
}

/// `MailSender` backed by the Resend HTTP API. The `from` address must use a
/// domain verified in the Resend dashboard (SPF/DKIM configured there).
pub struct ResendSender {
    api_key: String,
    from: String,
    http: reqwest::Client,
}

impl ResendSender {
    pub fn new(api_key: String, from: String) -> Self {
        Self {
            api_key,
            from,
            http: reqwest::Client::new(),
        }
    }
}

impl MailSender for ResendSender {
    fn send<'a>(
        &'a self,
        to: &'a str,
        subject: &'a str,
        body: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        // Own the string args so the returned future borrows only `self`
        // (its fields live for `'a`); passing `&'a str` straight through would
        // taint the future with the caller's borrow and break `Send`/lifetime
        // inference in `dyn` call sites.
        let to = to.to_string();
        let subject = subject.to_string();
        let body = body.to_string();
        Box::pin(async move {
            let payload = serde_json::json!({
                "from": self.from,
                "to": [to],
                "subject": subject,
                "text": body,
            });
            let response = self
                .http
                .post("https://api.resend.com/emails")
                .bearer_auth(&self.api_key)
                .json(&payload)
                .send()
                .await
                .map_err(|e| RelayError::Other(format!("resend request: {e}")))?;
            if response.status().is_success() {
                Ok(())
            } else {
                let status = response.status();
                let text = response.text().await.unwrap_or_default();
                Err(RelayError::Other(format!("resend {status}: {text}")))
            }
        })
    }
}
