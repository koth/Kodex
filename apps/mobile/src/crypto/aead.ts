import { chacha20poly1305 } from "@noble/ciphers/chacha";
import { gzipSync, gunzipSync } from "fflate";
import type { EncryptedEnvelope, Envelope } from "../types/relay-protocol";
import { PROTO_VERSION } from "../types/relay-protocol";
import { randomBytes } from "../util/random";

// ChaCha20-Poly1305 AEAD framing, byte-aligned with relay-client::crypto.
// Rust: fresh 12-byte OsRng nonce, AAD = to_device_id UTF-8 bytes, ciphertext =
// AEAD(plaintext = optionally gzip'd serde_json::to_vec(envelope)) + 16-byte
// Poly1305 tag. noble: chacha20poly1305(key, nonce, aad).encrypt(plaintext)
// appends the tag. Large payloads are gzip-compressed before encryption and
// flagged via EncryptedEnvelope.encoding ("gzip") so the receiver can invert
// it after decrypt (byte-aligned with relay-client::crypto::ENCODING_GZIP).

export const NONCE_LEN = 12;
export const TAG_LEN = 16;
/** Payload encoding marker stored on the wire in EncryptedEnvelope.encoding. */
export const ENCODING_GZIP = "gzip";
/** Serialized envelopes above this size are gzip-compressed before
 * encryption. Below it, compression overhead outweighs the savings. */
const COMPRESS_THRESHOLD_BYTES = 1024;

function toUtf8(s: string): Uint8Array {
  return new TextEncoder().encode(s);
}

/**
 * Encrypt a typed `Envelope` into a relay-routable `EncryptedEnvelope`.
 * `toDeviceId` is the routing target and is bound as AEAD associated data.
 * Large payloads are gzip-compressed first (flagged via `encoding`).
 */
export function encrypt(
  key: { bytes: Uint8Array },
  toDeviceId: string,
  envelope: Envelope,
): EncryptedEnvelope {
  const nonce = randomBytes(NONCE_LEN);
  const serialized = toUtf8(JSON.stringify(envelope));
  let plaintext = serialized;
  let encoding: string | undefined;
  if (serialized.length > COMPRESS_THRESHOLD_BYTES) {
    plaintext = gzipSync(serialized);
    encoding = ENCODING_GZIP;
  }
  const aad = toUtf8(toDeviceId);
  const cipher = chacha20poly1305(key.bytes, nonce, aad);
  const ciphertext = cipher.encrypt(plaintext);
  return {
    to_device_id: toDeviceId,
    nonce: Array.from(nonce),
    ciphertext: Array.from(ciphertext),
    ...(encoding ? { encoding } : {}),
  };
}

/**
 * Decrypt an `EncryptedEnvelope` back into a typed `Envelope`. Verifies the
 * AEAD tag, inverts the optional gzip payload encoding, and checks
 * `proto_version` matches the current `PROTO_VERSION`.
 */
export function decrypt(
  key: { bytes: Uint8Array },
  encrypted: EncryptedEnvelope,
): Envelope {
  if (encrypted.nonce.length !== NONCE_LEN) {
    throw new Error(`invalid nonce length: ${encrypted.nonce.length}`);
  }
  const nonce = new Uint8Array(encrypted.nonce);
  const aad = toUtf8(encrypted.to_device_id);
  const cipher = chacha20poly1305(key.bytes, nonce, aad);
  const plaintextBytes = cipher.decrypt(new Uint8Array(encrypted.ciphertext));
  let plaintext: string;
  switch (encrypted.encoding) {
    case undefined:
    case null:
      plaintext = new TextDecoder().decode(plaintextBytes);
      break;
    case ENCODING_GZIP:
      plaintext = new TextDecoder().decode(gunzipSync(plaintextBytes));
      break;
    default:
      throw new Error(`unknown payload encoding: ${encrypted.encoding}`);
  }
  const envelope: Envelope = JSON.parse(plaintext);
  if (envelope.proto_version !== PROTO_VERSION) {
    throw new Error(
      `proto version mismatch: got ${envelope.proto_version} expected ${PROTO_VERSION}`,
    );
  }
  return envelope;
}
