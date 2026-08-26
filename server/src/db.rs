use std::sync::{Arc, Mutex};

use rusqlite::{params, Connection, OptionalExtension};

use crate::errors::{RelayError, Result};

const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/0001_devices.sql"),
    include_str!("../migrations/0002_pairing_codes.sql"),
    include_str!("../migrations/0003_pairings.sql"),
    include_str!("../migrations/0004_accounts.sql"),
    include_str!("../migrations/0005_subscriptions.sql"),
    include_str!("../migrations/0006_login_codes.sql"),
    include_str!("../migrations/0007_accounts_email.sql"),
    include_str!("../migrations/0008_pairings_x25519.sql"),
];

#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl Db {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
        Self::init(conn)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> Result<Self> {
        // Track applied migrations via SQLite's `user_version` PRAGMA so the
        // server can restart safely. Without this, every start re-runs all
        // migrations and the non-idempotent `ALTER TABLE ... ADD COLUMN` in
        // migration 0007 fails with "duplicate column name: email".
        let current: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        for (idx, sql) in MIGRATIONS.iter().enumerate() {
            let version = (idx + 1) as u32;
            if version <= current {
                continue;
            }
            conn.execute_batch(sql)?;
            conn.pragma_update(None, "user_version", version)?;
        }
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Run a blocking rusqlite closure off the async runtime.
    pub(crate) async fn blocking<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> rusqlite::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let conn = self.conn.clone();
        let res = tokio::task::spawn_blocking(move || {
            let c = conn.lock().expect("db mutex poisoned");
            f(&c)
        })
        .await
        .map_err(|e| RelayError::Other(e.to_string()))?;
        res.map_err(RelayError::from)
    }

    // ---- devices ----

    pub async fn register_device(&self, device_id: String, public_key: String) -> Result<()> {
self.blocking(move |c| {
            c.execute(
                "INSERT OR IGNORE INTO devices (device_id, public_key, registered_at) \
                 VALUES (?1, ?2, ?3)",
                params![device_id, public_key, now_ms()],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn device_exists(&self, device_id: String) -> Result<bool> {
self.blocking(move |c| {
            let v: Option<i64> = c
                .query_row(
                    "SELECT 1 FROM devices WHERE device_id = ?1",
                    params![device_id],
                    |row| row.get(0),
                )
                .optional()?;
            Ok(v.is_some())
        })
        .await
    }

    // ---- pairing codes ----

    pub async fn register_pairing_code(
        &self,
        code: String,
        pc_device_id: String,
        ttl_secs: u64,
    ) -> Result<()> {
        let expires_at = now_ms() + (ttl_secs as i64) * 1000;
self.blocking(move |c| {
            // A PC may only have one live pairing code. Stale codes from an
            // earlier pairing (or an expired mint) would otherwise collide
            // with this INSERT on the `pairing_code` PRIMARY KEY — pairing
            // codes are random, so a collision with a *different* PC's code
            // is negligible, but a re-registration by the same PC within
            // the old code's TTL must always succeed.
            c.execute(
                "DELETE FROM pairing_codes WHERE pc_device_id = ?1",
                params![pc_device_id],
            )?;
            c.execute(
                "INSERT INTO pairing_codes \
                 (pairing_code, pc_device_id, created_at, expires_at, used) \
                 VALUES (?1, ?2, ?3, ?4, 0)",
                params![code, pc_device_id, now_ms(), expires_at],
            )?;
            Ok(())
        })
        .await
    }

    /// Returns the PC device_id bound to a pairing code if it is valid
    /// (exists, not expired, not used).
    pub async fn take_pairing_code(&self, code: String) -> Result<Option<String>> {
self.blocking(move |c| {
            let row: Option<(String, i64, i64)> = c
                .query_row(
                    "SELECT pc_device_id, expires_at, used FROM pairing_codes \
                     WHERE pairing_code = ?1",
                    params![code],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            Ok(match row {
                Some((pc, expires_at, used)) if used == 0 && expires_at >= now_ms() => Some(pc),
                _ => None,
            })
        })
        .await
    }

    pub async fn mark_pairing_code_used(&self, code: String) -> Result<()> {
self.blocking(move |c| {
            c.execute(
                "UPDATE pairing_codes SET used = 1 WHERE pairing_code = ?1",
                params![code],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn expire_pairing_codes(&self) -> Result<()> {
        let now = now_ms();
self.blocking(move |c| {
            c.execute(
                "DELETE FROM pairing_codes WHERE expires_at < ?1 OR used = 1",
                params![now],
            )?;
            Ok(())
        })
        .await
    }

    // ---- pairings ----

    pub async fn create_pairing(
        &self,
        pairing_id: String,
        pc_device_id: String,
        phone_device_id: String,
        pc_x25519_pubkey: String,
    ) -> Result<()> {
self.blocking(move |c| {
            c.execute(
                "INSERT INTO pairings \
                 (pairing_id, pc_device_id, phone_device_id, created_at, bound, account_id, \
                  pc_x25519_pubkey) \
                 VALUES (?1, ?2, ?3, ?4, 0, NULL, ?5)",
                params![pairing_id, pc_device_id, phone_device_id, now_ms(), pc_x25519_pubkey],
            )?;
            Ok(())
        })
        .await
    }

    /// True if `a` and `b` are paired together (either order).
    pub async fn pairing_for(&self, a: String, b: String) -> Result<Option<(String, String)>> {
self.blocking(move |c| {
            let row: Option<(String, String)> = c
                .query_row(
                    "SELECT pc_device_id, phone_device_id FROM pairings \
                     WHERE (pc_device_id = ?1 AND phone_device_id = ?2) \
                        OR (pc_device_id = ?2 AND phone_device_id = ?1)",
                    params![a, b],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            Ok(row)
        })
        .await
    }

    /// Latest pairing for a PC, by creation time. Used to resume when the
    /// phone's device identity rotated (secure-store miss / reinstall): the
    /// phone proves it holds the PC static public key by scanning, so the
    /// relay rebinds the newest pairing to the phone's fresh device id.
    pub async fn latest_pairing_for_pc(
        &self,
        pc_device_id: String,
    ) -> Result<Option<(String, String)>> {
        self.blocking(move |c| {
            let row: Option<(String, String)> = c
                .query_row(
                    "SELECT pairing_id, phone_device_id FROM pairings \
                     WHERE pc_device_id = ?1 ORDER BY created_at DESC LIMIT 1",
                    params![pc_device_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            Ok(row)
        })
        .await
    }

    /// Latest pairing partner of ANY device (PC or phone), by creation time.
    /// Used to route advisory `peer_session_reset` notices, which carry no
    /// explicit target: the relay delivers them to the sender's newest
    /// pairing partner.
    pub async fn latest_pairing_partner_for(
        &self,
        device_id: String,
    ) -> Result<Option<String>> {
        self.blocking(move |c| {
            let row: Option<String> = c
                .query_row(
                    "SELECT CASE WHEN pc_device_id = ?1 THEN phone_device_id \
                     ELSE pc_device_id END \
                     FROM pairings WHERE pc_device_id = ?1 OR phone_device_id = ?1 \
                     ORDER BY created_at DESC LIMIT 1",
                    params![device_id],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(row)
        })
        .await
    }

    /// Rebind a pairing to a new phone device id (identity rotation). The
    /// pairing token and PC side stay stable; the phone's fresh ephemeral
    /// resume then derives the E2E key with the PC static key it presented
    /// in the scanned QR.
    pub async fn update_pairing_phone(
        &self,
        pairing_id: String,
        phone_device_id: String,
    ) -> Result<()> {
        self.blocking(move |c| {
            c.execute(
                "UPDATE pairings SET phone_device_id = ?2 WHERE pairing_id = ?1",
                params![pairing_id, phone_device_id],
            )?;
            Ok(())
        })
        .await
    }

    /// Rebind every pairing for a PC to a new phone device id (identity
    /// rotation). Keeps older pairing tokens resumable after a re-scan: the
    /// phone's persisted `BoundDevice` references an older token whose PC
    /// side matches, and the phone proves control of the PC static key by
    /// having scanned the fresh QR.
    pub async fn rebind_pc_pairings_to_phone(
        &self,
        pc_device_id: String,
        phone_device_id: String,
    ) -> Result<usize> {
        self.blocking(move |c| {
            let n = c.execute(
                "UPDATE pairings SET phone_device_id = ?2 WHERE pc_device_id = ?1",
                params![pc_device_id, phone_device_id],
            )?;
            Ok(n)
        })
        .await
    }

    /// `(pairing_id, pc_device_id, phone_device_id)` for a device's pairing, if any.
    pub async fn pairing_id_for(
        &self,
        device_id: String,
    ) -> Result<Option<(String, String, String)>> {
self.blocking(move |c| {
            let row: Option<(String, String, String)> = c
                .query_row(
                    "SELECT pairing_id, pc_device_id, phone_device_id FROM pairings \
                     WHERE pc_device_id = ?1 OR phone_device_id = ?1",
                    params![device_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            Ok(row)
        })
        .await
    }

    /// The partner device_id for a paired device, if any.
    pub async fn partner_of(&self, device_id: String) -> Result<Option<String>> {
self.blocking(move |c| {
            let row: Option<(String, String)> = c
                .query_row(
                    "SELECT pc_device_id, phone_device_id FROM pairings \
                     WHERE pc_device_id = ?1 OR phone_device_id = ?1",
                    params![device_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            Ok(row.map(|(pc, ph)| if pc == device_id { ph } else { pc }))
        })
        .await
    }

    pub async fn bind_pairing(&self, pairing_id: String, account_id: String) -> Result<()> {
self.blocking(move |c| {
            c.execute(
                "UPDATE pairings SET bound = 1, account_id = ?2 WHERE pairing_id = ?1",
                params![pairing_id, account_id],
            )?;
            Ok(())
        })
        .await
    }

    // ---- accounts / subscriptions ----

    pub async fn account_by_token(&self, auth_token: String) -> Result<Option<String>> {
self.blocking(move |c| {
            let row: Option<String> = c
                .query_row(
                    "SELECT account_id FROM accounts WHERE auth_token = ?1",
                    params![auth_token],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(row)
        })
        .await
    }

    /// `(active, plan, expires_at)` for an account's subscription, if any.
    pub async fn subscription_status(
        &self,
        account_id: String,
    ) -> Result<Option<(bool, Option<String>, i64)>> {
self.blocking(move |c| {
            let row: Option<(i64, Option<String>, i64)> = c
                .query_row(
                    "SELECT active, plan, expires_at FROM subscriptions WHERE account_id = ?1",
                    params![account_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            Ok(row.map(|(active, plan, expires_at)| (active != 0, plan, expires_at)))
        })
        .await
    }

    /// All subscriptions marked active whose `expires_at` has passed:
    /// `(account_id, plan, expires_at)`.
    pub async fn expired_subscriptions(&self) -> Result<Vec<(String, Option<String>, i64)>> {
        let now = now_ms();
        self.blocking(move |c| {
            let mut stmt = c.prepare(
                "SELECT account_id, plan, expires_at FROM subscriptions \
                 WHERE active = 1 AND expires_at < ?1",
            )?;
            let rows = stmt.query_map(params![now], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
        .await
    }

    /// `(pc_device_id, phone_device_id)` for every pairing bound to an account.
    pub async fn pairings_for_account(&self, account_id: String) -> Result<Vec<(String, String)>> {
        self.blocking(move |c| {
            let mut stmt = c.prepare(
                "SELECT pc_device_id, phone_device_id FROM pairings WHERE account_id = ?1",
            )?;
            let rows = stmt.query_map(params![account_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
        .await
    }

    /// `(pc_device_id, phone_device_id, account_id)` for a bound pairing
    /// identified by its `pairing_id` (the `pairing_token` sent to peers).
    /// Returns `None` for an unknown or unbound pairing.
    pub async fn bound_pairing_by_token(
        &self,
        pairing_token: String,
    ) -> Result<Option<(String, String, String, String)>> {
        self.blocking(move |c| {
            let row: Option<(String, String, String, String)> = c
                .query_row(
                    "SELECT pc_device_id, phone_device_id, account_id, pc_x25519_pubkey FROM pairings \
                     WHERE pairing_id = ?1 AND bound = 1 AND account_id IS NOT NULL",
                    params![pairing_token],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?;
            Ok(row)
        })
        .await
    }

    /// `(pc_device_id, phone_device_id, account_id, pc_x25519_pubkey)` for any
    /// pairing by its `pairing_token` (bound or unbound). Resume does not
    /// require an active subscription; it only proves the two already-paired
    /// device identities are the same, and the fresh E2E key is re-derived
    /// by the peers from their private keys.
    pub async fn pairing_by_token(
        &self,
        pairing_token: String,
    ) -> Result<Option<(String, String, Option<String>, String)>> {
        self.blocking(move |c| {
            let row: Option<(String, String, Option<String>, String)> = c
                .query_row(
                    "SELECT pc_device_id, phone_device_id, account_id, pc_x25519_pubkey FROM pairings \
                     WHERE pairing_id = ?1",
                    params![pairing_token],
                    |r| {
                        Ok((
                            r.get(0)?,
                            r.get(1)?,
                            r.get::<_, Option<String>>(2)?,
                            r.get(3)?,
                        ))
                    },
                )
                .optional()?;
            Ok(row)
        })
        .await
    }

    pub async fn deactivate_subscription(&self, account_id: String) -> Result<()> {
        self.blocking(move |c| {
            c.execute(
                "UPDATE subscriptions SET active = 0 WHERE account_id = ?1",
                params![account_id],
            )?;
            Ok(())
        })
        .await
    }

    // ---- login codes (passwordless email OTP) ----

    pub async fn upsert_login_code(&self, email: String, code: String) -> Result<()> {
        self.blocking(move |c| {
            c.execute(
                "INSERT INTO login_codes (email, code, issued_at, attempts, consumed) \
                 VALUES (?1, ?2, ?3, 0, 0) \
                 ON CONFLICT(email) DO UPDATE SET \
                   code = excluded.code, issued_at = excluded.issued_at, \
                   attempts = 0, consumed = 0",
                params![email, code, now_ms()],
            )?;
            Ok(())
        })
        .await
    }

    /// `(code, issued_at, attempts, consumed)` for the email's in-flight code.
    pub async fn login_code(
        &self,
        email: String,
    ) -> Result<Option<(String, i64, i32, i32)>> {
        self.blocking(move |c| {
            let row: Option<(String, i64, i32, i32)> = c
                .query_row(
                    "SELECT code, issued_at, attempts, consumed FROM login_codes \
                     WHERE email = ?1",
                    params![email],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?;
            Ok(row)
        })
        .await
    }

    pub async fn increment_login_attempt(&self, email: String) -> Result<()> {
        self.blocking(move |c| {
            c.execute(
                "UPDATE login_codes SET attempts = attempts + 1 WHERE email = ?1",
                params![email],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn consume_login_code(&self, email: String) -> Result<()> {
        self.blocking(move |c| {
            c.execute(
                "UPDATE login_codes SET consumed = 1 WHERE email = ?1",
                params![email],
            )?;
            Ok(())
        })
        .await
    }

    // ---- accounts: passwordless login session ----

    pub async fn account_id_by_email(&self, email: String) -> Result<Option<String>> {
        self.blocking(move |c| {
            let row: Option<String> = c
                .query_row(
                    "SELECT account_id FROM accounts WHERE email = ?1",
                    params![email],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(row)
        })
        .await
    }

    /// Create or refresh a session auth_token for the account bound to
    /// `email`. An existing account keeps its stable `account_id` but gets a
    /// fresh token (the previous token is invalidated, so prior bound
    /// pairings must re-bind with the new token). Returns
    /// `(account_id, auth_token)`.
    pub async fn issue_account_session(&self, email: String) -> Result<(String, String)> {
        let auth_token = uuid::Uuid::new_v4().to_string();
        let new_account_id = uuid::Uuid::new_v4().to_string();
        self.blocking({
            // Clone into the 'static closure; the originals are returned /
            // reused on the outer task below.
            let email = email.clone();
            let auth_token = auth_token.clone();
            let new_account_id = new_account_id.clone();
            move |c| {
                let updated = c.execute(
                    "UPDATE accounts SET auth_token = ?1 WHERE email = ?2",
                    params![auth_token, email],
                )?;
                if updated == 0 {
                    c.execute(
                        "INSERT INTO accounts (account_id, credentials, auth_token, email) \
                         VALUES (?1, ?2, ?3, ?4)",
                        params![new_account_id, "{}", auth_token, email],
                    )?;
                }
                Ok(())
            }
        })
        .await?;
        let id = self
            .account_id_by_email(email.clone())
            .await?
            .unwrap_or(new_account_id);
        Ok((id, auth_token))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn migrations_and_pairing_code_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        db.register_device("pc-1".into(), "pk".into()).await.unwrap();
        assert!(db.device_exists("pc-1".into()).await.unwrap());
        assert!(!db.device_exists("unknown".into()).await.unwrap());

        db.register_pairing_code("ABCD2345".into(), "pc-1".into(), 120)
            .await
            .unwrap();
        let pc = db.take_pairing_code("ABCD2345".into()).await.unwrap();
        assert_eq!(pc.as_deref(), Some("pc-1"));
        db.mark_pairing_code_used("ABCD2345".into()).await.unwrap();
        let again = db.take_pairing_code("ABCD2345".into()).await.unwrap();
        assert_eq!(again, None);
    }

    #[tokio::test]
    async fn pairing_partner_lookup() {
        let db = Db::open_in_memory().unwrap();
        db.register_device("pc".into(), "pk".into()).await.unwrap();
        db.register_device("ph".into(), "pk2".into()).await.unwrap();
        db.create_pairing("p-1".into(), "pc".into(), "ph".into(), "pc-x".into())
            .await
            .unwrap();
        assert_eq!(
            db.partner_of("pc".into()).await.unwrap().as_deref(),
            Some("ph")
        );
        assert_eq!(
            db.partner_of("ph".into()).await.unwrap().as_deref(),
            Some("pc")
        );
        assert!(db.pairing_for("pc".into(), "ph".into()).await.unwrap().is_some());
        assert!(db.pairing_for("pc".into(), "other".into()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn subscription_status_lookup() {
        let db = Db::open_in_memory().unwrap();
        db.blocking(|c| {
            c.execute(
                "INSERT INTO accounts (account_id, credentials, auth_token) VALUES (?1, ?2, ?3)",
                params!["acct-1", "{}", "tok-1"],
            )?;
            c.execute(
                "INSERT INTO subscriptions (account_id, plan, active, expires_at) \
                 VALUES (?1, ?2, 1, 2000000000)",
                params!["acct-1", "monthly"],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let acct = db.account_by_token("tok-1".into()).await.unwrap();
        assert_eq!(acct.as_deref(), Some("acct-1"));
        let sub = db.subscription_status("acct-1".into()).await.unwrap();
        let (active, plan, _exp) = sub.unwrap();
        assert!(active);
        assert_eq!(plan.as_deref(), Some("monthly"));
    }
}
