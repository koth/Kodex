-- pairings: persist the PC static X25519 public key so a bound phone can
-- resume E2E key derivation without re-scanning the QR (a fresh ephemeral
-- phone key is still generated on each resume).
ALTER TABLE pairings ADD COLUMN pc_x25519_pubkey TEXT;
