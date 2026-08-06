-- accounts: add a unique email column for the passwordless login flow.
-- `credentials` is kept for future password/IdP metadata; the email is the
-- login identity and is looked up on code verification to upsert the account.
ALTER TABLE accounts ADD COLUMN email TEXT;
CREATE UNIQUE INDEX IF NOT EXISTS idx_accounts_email ON accounts(email) WHERE email IS NOT NULL;
UPDATE accounts SET email = NULL;
