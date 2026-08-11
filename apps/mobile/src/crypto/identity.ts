import { sha256 } from "@noble/hashes/sha256";
import { ed25519 } from "@noble/curves/ed25519";
import { encodeBase64UrlNoPad } from "../util/base64url";
import {
  generatePrivateKey,
  getPublicKey,
  SECRET_KEY_LEN,
} from "./ecdh";

// Device identity, byte-aligned with relay_client::identity::DeviceIdentity.
// Two independent keypairs:
// - X25519 static keypair: the E2E key-exchange key (pairing ECDH). Never
//   used for signing.
// - Ed25519 signing keypair: authenticates to the relay. The device signs
//   `{device_id}:{ts_ms}` with the Ed25519 secret; the relay verifies with
//   the Ed25519 public key carried in DeviceAuth.
// device_id = base64url-no-pad(SHA-256(x25519_public_key))   [URL_SAFE_NO_PAD]
// auth_signature = base64url-no-pad(Ed25519.sign("{device_id}:{ts_ms}", ed25519_secret))
// The X25519 static keypair is the DEVICE identity (persistent, secure store);
// it is distinct from the per-pairing ephemeral key used for E2E derivation.
// Persistence: 64 bytes = x25519_secret (32) || ed25519_secret (32). Legacy
// 32-byte entries are migrated by generating a fresh Ed25519 keypair.

export interface DeviceIdentity {
  /** 32-byte static X25519 private key (never transmitted in plaintext). */
  readonly secret: Uint8Array;
  /** 32-byte static public key. */
  readonly publicKey: Uint8Array;
  /** 32-byte Ed25519 signing secret (never transmitted). */
  readonly signingSecret: Uint8Array;
  /** 32-byte Ed25519 verifying (public) key. */
  readonly signingPublicKey: Uint8Array;
}

/**
 * Pluggable secret persistence so the identity core stays unit-testable without
 * the platform secure-storage (expo-secure-store) dependency. The app wires the
 * Keychain/Keystore-backed implementation; tests use an in-memory store.
 */
export interface SecretStore {
  get(key: string): Promise<Uint8Array | null>;
  set(key: string, value: Uint8Array): Promise<void>;
  delete(key: string): Promise<void>;
}

export const DEVICE_SECRET_KEY = "kodex.device-secret";

/** 64-byte persisted secret blob: x25519_secret (32) || ed25519_secret (32). */
export const DEVICE_SECRET_LEN = 64;

/** Generate a fresh device identity (OS RNG): X25519 + Ed25519 keypairs. */
export function generateDeviceIdentity(): DeviceIdentity {
  const secret = generatePrivateKey();
  const publicKey = getPublicKey(secret);
  const signingSecret = generatePrivateKey();
  const signingPublicKey = ed25519.getPublicKey(signingSecret);
  return { secret, publicKey, signingSecret, signingPublicKey };
}

/** Reconstruct an identity from a stored secret blob. Accepts 64 bytes
 * (x25519 || ed25519) or a legacy 32-byte (x25519-only) entry, generating a
 * fresh Ed25519 keypair for the latter. Derives both public keys. */
export function deviceIdentityFromSecret(bytes: Uint8Array): DeviceIdentity {
  if (bytes.length === DEVICE_SECRET_LEN) {
  const secret = bytes.subarray(0, SECRET_KEY_LEN);
  const signingSecret = bytes.subarray(SECRET_KEY_LEN, DEVICE_SECRET_LEN);
  return {
    secret,
    publicKey: getPublicKey(secret),
    signingSecret,
    signingPublicKey: ed25519.getPublicKey(signingSecret),
  };
  }
  if (bytes.length === SECRET_KEY_LEN) {
  // Legacy 32-byte X25519-only entry: keep the X25519 key, mint Ed25519.
  const signingSecret = generatePrivateKey();
  return {
    secret: bytes,
    publicKey: getPublicKey(bytes),
    signingSecret,
    signingPublicKey: ed25519.getPublicKey(signingSecret),
  };
  }
  throw new Error(
  `device secret is ${bytes.length} bytes, expected ${SECRET_KEY_LEN} or ${DEVICE_SECRET_LEN}`,
  );
}

/** 64-byte secret blob for persistence: x25519_secret || ed25519_secret. */
export function deviceSecretBytes(identity: DeviceIdentity): Uint8Array {
  const out = new Uint8Array(DEVICE_SECRET_LEN);
  out.set(identity.secret, 0);
  out.set(identity.signingSecret, SECRET_KEY_LEN);
  return out;
}

/** Stable device id: base64url-no-pad(SHA-256(x25519_public_key)). */
export function deviceId(identity: DeviceIdentity): string {
  const hash = sha256(identity.publicKey);
  return encodeBase64UrlNoPad(hash);
}

/** X25519 public key, base64url-no-pad (for the QR pairing payload). */
export function publicKeyB64(identity: DeviceIdentity): string {
  return encodeBase64UrlNoPad(identity.publicKey);
}

/** Ed25519 verifying (public) key, base64url-no-pad. Sent in DeviceAuth so
 * the relay can verify `authSignature`. */
export function devicePubkeyB64(identity: DeviceIdentity): string {
  return encodeBase64UrlNoPad(identity.signingPublicKey);
}

/** Ed25519 signature over `{device_id}:{timestamp_ms}`, base64url-no-pad.
 * The relay verifies this with `devicePubkeyB64`. */
export function authSignature(identity: DeviceIdentity, timestampMs: number): string {
  const message = `${deviceId(identity)}:${timestampMs}`;
  const sig = ed25519.sign(new TextEncoder().encode(message), identity.signingSecret);
  return encodeBase64UrlNoPad(sig);
}

/**
 * Load the device identity from `store`, generating + persisting a fresh one
 * if absent. Mirrors relay_client::DeviceIdentity::load_or_create.
 */
export async function loadOrCreateIdentity(
  store: SecretStore,
): Promise<DeviceIdentity> {
  const existing = await store.get(DEVICE_SECRET_KEY);
  if (existing) {
  const identity = deviceIdentityFromSecret(existing);
  // Re-persist if we migrated a legacy 32-byte entry so future loads
  // read 64 bytes directly.
  if (existing.length !== DEVICE_SECRET_LEN) {
  await store.set(DEVICE_SECRET_KEY, deviceSecretBytes(identity));
  }
  return identity;
  }
  const identity = generateDeviceIdentity();
  await store.set(DEVICE_SECRET_KEY, deviceSecretBytes(identity));
  return identity;
}
