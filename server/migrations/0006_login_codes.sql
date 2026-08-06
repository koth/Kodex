-- login_codes: email verification codes for passwordless login.
-- One in-flight code per email (INSERT OR REPLACE on re-issue). Codes are
-- short-lived (TTL enforced in app logic), one-time (consumed on success),
-- and attempt-capped. Stored in plaintext because they are single-use and
-- expire within minutes; password hashing (argon2) is reserved for a future
-- password-based auth mode, not this OTP flow.
CREATE TABLE IF NOT EXISTS login_codes (
    email      TEXT PRIMARY KEY,
    code       TEXT NOT NULL,
    issued_at  INTEGER NOT NULL,
    attempts   INTEGER NOT NULL DEFAULT 0,
    consumed   INTEGER NOT NULL DEFAULT 0
);
