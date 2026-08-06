//! Passwordless email-OTP login orchestration.
//!
//! Two-step flow:
//! 1. `send_code`: mint a 6-digit code, persist it (short-lived, one-time,
//!    attempt-capped), and email it via `MailSender`.
//! 2. `verify_and_login`: validate the code, consume it, and issue a fresh
//!    account session (`account_id` + `auth_token`) that the existing
//!    `BindDeviceRequest` flow already consumes via `account_by_token`.
//!
//! The wire protocol is untouched: `BindDeviceRequest { auth_token }` keeps
//! working because `auth_token` stays an opaque string — only its minting
//! source changed from a placeholder to this login flow.

use crate::db::Db;
use crate::errors::{RelayError, Result};
use crate::mail::MailSender;

const CODE_TTL_SECS: u64 = 600;
const CODE_LEN: usize = 6;
const MAX_ATTEMPTS: i32 = 5;
const ISSUE_COOLDOWN_SECS: u64 = 60;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// A 6-digit one-time code derived from a v4 UUID (cryptographically random,
/// so no separate `rand` dependency). Zero-padded to `CODE_LEN`.
fn gen_code() -> String {
    let n = uuid::Uuid::new_v4().as_u128() % 1_000_000;
    format!("{:01$}", n, CODE_LEN)
}

/// Mint and email a verification code. Rate-limited per email via a
/// cooldown so a user (or attacker) cannot spam the relay into bulk-sending.
pub async fn send_code(
    db: &Db,
    mail: &dyn MailSender,
    email: &str,
) -> Result<()> {
    let cooldown_ms = (ISSUE_COOLDOWN_SECS as i64) * 1000;
    if let Some((_, issued_at, _, _)) = db.login_code(email.to_string()).await? {
        if now_ms() - issued_at < cooldown_ms {
            return Err(RelayError::Other("请求过于频繁，请稍后再试".into()));
        }
    }
    let code = gen_code();
    db.upsert_login_code(email.to_string(), code.clone())
        .await?;
    let body = format!(
        "你的 Kodex 登录验证码是 {code}，{} 分钟内有效。如非本人操作请忽略。",
        CODE_TTL_SECS / 60
    );
    mail.send(email, "Kodex 登录验证码", &body)
        .await?;
    Ok(())
}

/// Validate the code and issue a session token. On success returns
/// `(account_id, auth_token)`; the existing bind flow resolves the account
/// via `account_by_token(auth_token)`.
pub async fn verify_and_login(
    db: &Db,
    email: &str,
    code: &str,
) -> Result<(String, String)> {
    let record = db
        .login_code(email.to_string())
        .await?
        .ok_or_else(|| RelayError::Other("验证码不存在或已过期，请重新获取".into()))?;
    let (stored, issued_at, attempts, consumed) = record;
    if consumed != 0 {
        return Err(RelayError::Other("验证码已使用，请重新获取".into()));
    }
    let ttl_ms = (CODE_TTL_SECS as i64) * 1000;
    if now_ms() - issued_at > ttl_ms {
        return Err(RelayError::Other("验证码已过期，请重新获取".into()));
    }
    if attempts >= MAX_ATTEMPTS {
        return Err(RelayError::Other("尝试次数过多，请重新获取验证码".into()));
    }
    if !code.eq(&stored) {
        db.increment_login_attempt(email.to_string()).await?;
        return Err(RelayError::Other("验证码错误".into()));
    }
    db.consume_login_code(email.to_string()).await?;
    db.issue_account_session(email.to_string()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MockMail {
        captured: Mutex<Vec<(String, String, String)>>,
    }

    impl MailSender for MockMail {
        fn send<'a>(
            &'a self,
            to: &'a str,
            subject: &'a str,
            body: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
            let to = to.to_string();
            let subject = subject.to_string();
            let body = body.to_string();
            Box::pin(async move {
                self.captured
                    .lock()
                    .expect("mock mail mutex")
                    .push((to, subject, body));
                Ok(())
            })
        }
    }

    fn db() -> Db {
        Db::open_in_memory().expect("in-memory db")
    }

    /// Insert a code with an explicit `issued_at` (ms) to test TTL/cooldown
    /// without waiting for real time.
    async fn seed_code(db: &Db, email: &str, code: &str, issued_at: i64) {
        let email = email.to_string();
        let code = code.to_string();
        db.blocking(move |c| {
            c.execute(
                "INSERT INTO login_codes (email, code, issued_at, attempts, consumed) \
                 VALUES (?1, ?2, ?3, 0, 0) \
                 ON CONFLICT(email) DO UPDATE SET \
                   code = excluded.code, issued_at = excluded.issued_at, \
                   attempts = 0, consumed = 0",
                rusqlite::params![email, code, issued_at],
            )?;
            Ok(())
        })
        .await
        .expect("seed code");
    }

    async fn current_code(db: &Db, email: &str) -> String {
        let (code, _, _, _) = db
            .login_code(email.to_string())
            .await
            .unwrap()
            .expect("code exists");
        code
    }

    #[tokio::test]
    async fn send_code_emails_a_six_digit_code() {
        let db = db();
        let mail = MockMail::default();
        send_code(&db, &mail, "user@example.com").await.unwrap();
        let code = current_code(&db, "user@example.com").await;
        assert_eq!(code.len(), CODE_LEN);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
        let (to, _subject, body) = &mail.captured.lock().unwrap()[0];
        assert_eq!(to, "user@example.com");
        assert!(body.contains(&code));
    }

    #[tokio::test]
    async fn send_code_cooldown_blocks_rapid_resend() {
        let db = db();
        let mail = MockMail::default();
        send_code(&db, &mail, "user@example.com").await.unwrap();
        let err = send_code(&db, &mail, "user@example.com")
            .await
            .expect_err("cooldown should block");
        assert!(err.to_string().contains("请求过于频繁"));
    }

    #[tokio::test]
    async fn wrong_code_increments_attempts_then_locks_out() {
        let db = db();
        seed_code(&db, "u@e.com", "123456", now_ms()).await;
        for _ in 0..MAX_ATTEMPTS {
            let err = verify_and_login(&db, "u@e.com", "000000")
                .await
                .expect_err("wrong code");
            assert!(err.to_string().contains("验证码错误"));
        }
        let err = verify_and_login(&db, "u@e.com", "000000")
            .await
            .expect_err("locked out");
        assert!(err.to_string().contains("尝试次数过多"));
    }

    #[tokio::test]
    async fn expired_code_is_rejected() {
        let db = db();
        let stale = now_ms() - (CODE_TTL_SECS as i64 + 60) * 1000;
        seed_code(&db, "u@e.com", "123456", stale).await;
        let err = verify_and_login(&db, "u@e.com", "123456")
            .await
            .expect_err("expired");
        assert!(err.to_string().contains("已过期"));
    }

    #[tokio::test]
    async fn success_consumes_code_and_mints_token() {
        let db = db();
        seed_code(&db, "u@e.com", "123456", now_ms()).await;
        let (account_id, token) = verify_and_login(&db, "u@e.com", "123456")
            .await
            .expect("valid code");
        assert!(!account_id.is_empty());
        assert!(!token.is_empty());

        // The account row is backed by email; a second login keeps the same
        // account_id but rotates the token.
        seed_code(&db, "u@e.com", "654321", now_ms()).await;
        let (account_id_2, token_2) = verify_and_login(&db, "u@e.com", "654321")
            .await
            .expect("valid second code");
        assert_eq!(account_id_2, account_id, "account_id is stable per email");
        assert_ne!(token_2, token, "token rotates on each login");

        // The consumed code cannot be replayed.
        let err = verify_and_login(&db, "u@e.com", "654321")
            .await
            .expect_err("replay");
        assert!(err.to_string().contains("已使用"));
    }

    #[tokio::test]
    async fn minted_token_resolves_via_account_by_token() {
        let db = db();
        seed_code(&db, "u@e.com", "123456", now_ms()).await;
        let (account_id, token) = verify_and_login(&db, "u@e.com", "123456")
            .await
            .unwrap();
        let resolved = db.account_by_token(token).await.unwrap();
        assert_eq!(resolved.as_deref(), Some(account_id.as_str()));
    }
}
