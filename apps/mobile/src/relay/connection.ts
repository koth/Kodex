import type { RelayTransport } from "./transport";
import type { Envelope, EncryptedEnvelope, Message } from "../types/relay-protocol";
import { PROTO_VERSION, DeviceAuth } from "../types/relay-protocol";
import { encrypt, decrypt } from "../crypto/aead";
import type { SessionKey } from "../crypto/session-key";
import { serializeEnvelope, parseEnvelope } from "./framing";
import { diagnostics } from "../util/diagnostics";

// A relay connection with optional E2E encryption. When no session key is
// installed (pre-pairing auth phase) it sends/receives plain `Envelope` JSON;
// after `installSessionKey` it encrypts each `Envelope` into an
// `EncryptedEnvelope` and decrypts inbound frames, so the relay routes
// ciphertext only. Mirrors relay_client::connection::RelayConnection.
export class RelayConnection {
  private sessionKey: { bytes: Uint8Array } | null = null;
  private peerDeviceId: string | null = null;

  constructor(
    private readonly transport: RelayTransport,
    readonly heartbeatMs: number = 30_000,
  ) {}

  /** Install the E2E session key (post-pairing). Subsequent send/recv is E2E. */
  installSessionKey(key: SessionKey, peerDeviceId: string): void {
    this.sessionKey = key;
    this.peerDeviceId = peerDeviceId;
  }

  hasSessionKey(): boolean {
    return this.sessionKey !== null;
  }

  getSessionKey(): { bytes: Uint8Array } | null {
    return this.sessionKey;
  }

  getPeerDeviceId(): string | null {
    return this.peerDeviceId;
  }

  /** Send an envelope: encrypt to EncryptedEnvelope when a key is installed,
   * otherwise send plain Envelope JSON (auth phase). */
  async sendEnvelope(envelope: Envelope): Promise<void> {
    const frame =
      this.sessionKey && this.peerDeviceId
        ? serializeEncrypted(
            encrypt(this.sessionKey, this.peerDeviceId, envelope),
          )
        : serializeEnvelope(envelope);
    diagnostics.log("conn", `send ${envelope.type} encrypted=${!!this.sessionKey}`);
    await this.transport.sendText(frame);
  }

  /** Send an envelope as plaintext regardless of the installed session key.
   * Used for heartbeats, which the relay must consume itself (an encrypted
   * frame would be routed to the peer and never counted for liveness). */
  async sendPlaintext(envelope: Envelope): Promise<void> {
    await this.transport.sendText(serializeEnvelope(envelope));
  }

  /** Receive the next envelope: decrypt an EncryptedEnvelope when a key is
   * installed, otherwise parse a plain Envelope. Returns null on clean close. */
  async recvEnvelope(): Promise<Envelope | null> {
    const frame = await this.transport.recvText();
    if (frame === null) {
      console.log("[conn] recv closed");
      return null;
    }
    if (this.sessionKey) {
      // Mixed framing: relay-originated frames (SubscriptionStatus acks,
      // pairing errors) are always plaintext — the relay holds no E2E key.
      // Route on shape instead of assuming everything is encrypted.
      if (!frame.includes('"to_device_id"')) {
        diagnostics.log("conn", "recv plain (relay-originated, key installed)");
        return parseEnvelope(frame);
      }
      const enc = JSON.parse(frame) as EncryptedEnvelope;
      try {
        const env = decrypt(this.sessionKey, enc);
        diagnostics.log("conn", `recv ${env.type} decrypted=ok`);
        return env;
      } catch (e) {
        diagnostics.log("conn", `recv decrypt FAILED: ${e}`);
        throw e;
      }
    }
    diagnostics.log("conn", "recv plain");
    return parseEnvelope(frame);
  }

  /** Pre-pairing auth: send a DeviceAuth envelope (plain) and await an ack.
   * Must be called before installSessionKey. `devicePubkey` is the Ed25519
   * verifying key (base64url-no-pad) the relay verifies `signature` with. */
  async authenticate(
    deviceId: string,
    devicePubkey: string,
    signature: string,
    timestampMs: number,
    timeoutMs = 15_000,
  ): Promise<void> {
    const auth: DeviceAuth = {
      device_id: deviceId,
      device_pubkey: devicePubkey,
      signature,
      timestamp_ms: timestampMs,
    };
    const env: Envelope = {
      proto_version: PROTO_VERSION,
      id: null,
      type: "device_auth",
      payload: auth,
    };
    await this.sendEnvelope(env);
    const ack = await this.recvEnvelopeWithTimeout(timeoutMs);
    if (ack === null) {
      throw new Error("relay closed during auth handshake");
    }
    const msg = ack;
    if (msg.type !== "device_auth" && msg.type !== "subscription_status") {
      throw new Error(`unexpected auth response: ${msg.type}`);
    }
  }

  private async recvEnvelopeWithTimeout(timeoutMs: number): Promise<Envelope | null> {
    return new Promise((resolve, reject) => {
      const timer = setTimeout(
        () => reject(new Error("认证超时：relay 未响应，请检查 PC/relay 是否在线")),
        timeoutMs,
      );
      this.recvEnvelope()
        .then((value) => {
          clearTimeout(timer);
          resolve(value);
        })
        .catch((error) => {
          clearTimeout(timer);
          reject(error);
        });
    });
  }

  async close(): Promise<void> {
    await this.transport.close();
  }
}

function serializeEncrypted(enc: EncryptedEnvelope): string {
  return JSON.stringify(enc);
}
// end of file
