import type { RelayConnection } from "../relay/connection";
import type {
  PairingQrPayload,
  PairingConfirm,
  PairingResume,
} from "../types/relay-protocol";
import { fromMessage } from "../relay/framing";
import { PROTO_VERSION } from "../types/relay-protocol";
import type { DeviceIdentity } from "../crypto/identity";
import { deviceId, authSignature, devicePubkeyB64, publicKeyB64 } from "../crypto/identity";
import { generatePrivateKey, getPublicKey, ecdhSharedSecret } from "../crypto/ecdh";
import { decodeBase64UrlNoPad, encodeBase64UrlNoPad } from "../util/base64url";
import { deriveSessionKey, type SessionKey } from "../crypto/session-key";
import { pcStaticPublicKey } from "./qr-parse";
import type { BoundDevice } from "../account/binding";

const PAIRING_HANDSHAKE_TIMEOUT_MS = 30_000;

async function recvEnvelopeWithTimeout(
  conn: RelayConnection,
  timeoutMs: number = PAIRING_HANDSHAKE_TIMEOUT_MS,
): Promise<ReturnType<RelayConnection["recvEnvelope"]>> {
  const timeout = new Promise<null>((_, reject) => {
    setTimeout(() => reject(new Error("配对超时：二维码可能已过期，请刷新 PC 端二维码后重试")), timeoutMs);
  });
  return Promise.race([conn.recvEnvelope(), timeout]);
}

export interface PairingResult {
  sessionKey: SessionKey;
  pcDeviceId: string;
  phoneDeviceId: string;
  pairingToken: string;
  /** PC static X25519 public key, base64url-no-pad, for bound reconnect. */
  pcStaticPubkeyB64: string;
}

/**
 * Run the phone side of the E2E pairing handshake over an already-dialed,
 * authenticated connection (DeviceAuth happens first, in the connection layer).
 * Generates a fresh ephemeral X25519 keypair, sends `PairingInitiate` with the
 * ephemeral public key, awaits `PairingConfirm`, and derives the SessionKey
 * from X25519(ephemeral_secret, pc_static_public) + HKDF. The ephemeral secret
 * and pairing code are discarded after derivation (security rule).
 */
export async function runPairingHandshake(
  conn: RelayConnection,
  identity: DeviceIdentity,
  qr: PairingQrPayload,
): Promise<PairingResult> {
  // Fresh ephemeral keypair for this pairing (discarded after derivation).
  const ephemeralSecret = generatePrivateKey();
  const ephemeralPublic = getPublicKey(ephemeralSecret);
  const pcStaticPublic = pcStaticPublicKey(qr);

  const initiateEnv = fromMessage(null, {
  type: "pairing_initiate",
  payload: {
  pairing_code: qr.pairing_code,
  pc_device_pubkey: qr.pc_device_pubkey,
  relay_endpoint: qr.relay_endpoint,
  phone_ephemeral_pubkey: encodeBase64UrlNoPad(ephemeralPublic),
  },
  });
  await conn.sendEnvelope(initiateEnv);

  const confirmEnv = await recvEnvelopeWithTimeout(conn);
  if (confirmEnv === null) {
  throw new Error("relay closed during pairing handshake");
  }
  if (confirmEnv.type !== "pairing_confirm") {
  throw new Error(`unexpected pairing response: ${confirmEnv.type}`);
  }
  const confirm = confirmEnv.payload as PairingConfirm;

  const shared = ecdhSharedSecret(ephemeralSecret, pcStaticPublic);
  const sessionKey = deriveSessionKey(shared);

  return {
  sessionKey,
  pcDeviceId: confirm.pc_device_id,
  phoneDeviceId: confirm.phone_device_id,
  pairingToken: confirm.pairing_token,
  pcStaticPubkeyB64: qr.pc_device_pubkey,
  };
}

/**
 * Bound-account resume: the phone already has a persisted `BoundDevice`; it
 * mints a fresh ephemeral X25519 keypair, asks the relay to forward the fresh
 * public key to the paired PC, and derives a fresh session key from the PC's
 * stored static public key. The ephemeral secret is discarded after use.
 */
export async function runPairingResume(
  conn: RelayConnection,
  bound: BoundDevice,
): Promise<{ sessionKey: SessionKey; pcDeviceId: string }> {
  const pcStaticPubB64 = bound.peer_static_pubkey_b64;
  if (!pcStaticPubB64) {
    throw new Error("bound device is missing the PC static public key; re-scan required");
  }
  const ephemeralSecret = generatePrivateKey();
  const phoneEphPub = getPublicKey(ephemeralSecret);
  const pcStaticPub = decodeBase64UrlNoPad(pcStaticPubB64);

  const resume: PairingResume = {
    pairing_token: bound.pairing_token,
    phone_ephemeral_pubkey: encodeBase64UrlNoPad(phoneEphPub),
  };
  const env = fromMessage(null, { type: "pairing_resume", payload: resume });
  await conn.sendEnvelope(env);

  const confirmEnv = await recvEnvelopeWithTimeout(conn);
  if (confirmEnv === null) {
    throw new Error("relay closed during pairing resume");
  }
  if (confirmEnv.type !== "pairing_confirm") {
    throw new Error(`unexpected pairing resume response: ${confirmEnv.type}`);
  }
  const confirm = confirmEnv.payload as PairingConfirm;

  const shared = ecdhSharedSecret(ephemeralSecret, pcStaticPub);
  return {
    sessionKey: deriveSessionKey(shared),
    pcDeviceId: confirm.pc_device_id,
  };
}

/** Build a DeviceAuth envelope for the connection-layer authenticate step. */
export function buildDeviceAuthArgs(identity: DeviceIdentity): {
  deviceId: string;
  devicePubkey: string;
  signature: string;
  timestampMs: number;
} {
  const ts = Date.now();
  return {
  deviceId: deviceId(identity),
  devicePubkey: devicePubkeyB64(identity),
  signature: authSignature(identity, ts),
  timestampMs: ts,
  };
}

/** Re-export the public-key b64 helper for tests/parity checks. */
export { publicKeyB64 };

/** A pairing code is single-use: this helper signals it must not be reused. */
export const PAIRING_CODE_SINGLE_USE = true;

// Re-export PROTO_VERSION for callers building raw envelopes.
export { PROTO_VERSION };
// end of file
