import type { RelayTransport } from "./transport";
import type { Envelope, EncryptedEnvelope, Message } from "../types/relay-protocol";
import { PROTO_VERSION, DeviceAuth } from "../types/relay-protocol";
import { encrypt, decrypt } from "../crypto/aead";
import type { SessionKey } from "../crypto/session-key";
import { serializeEnvelope, parseEnvelope } from "./framing";
import { uuidV4 } from "../util/uuid";
import { diagnostics } from "../util/diagnostics";

/** Single encrypted frames above this serialized size are split into chunks. */
const CHUNK_SINGLE_FRAME_BYTES = 256 * 1024;
/** Target ciphertext bytes per chunk (well under relay/mobile WS limits). */
const CHUNK_PAYLOAD_BYTES = 128 * 1024;

interface ChunkBuffer {
  total: number;
  received: (Uint8Array | null)[];
  nonce: number[];
  toDeviceId: string;
}

// A relay connection with optional E2E encryption. When no session key is
// installed (pre-pairing auth phase) it sends/receives plain `Envelope` JSON;
// after `installSessionKey` it encrypts each `Envelope` into an
// `EncryptedEnvelope` and decrypts inbound frames, so the relay routes
// ciphertext only. Mirrors relay_client::connection::RelayConnection.
export class RelayConnection {
  private sessionKey: { bytes: Uint8Array } | null = null;
  private peerDeviceId: string | null = null;
  private chunks = new Map<string, ChunkBuffer>();

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
   * otherwise send plain Envelope JSON (auth phase). Large encrypted
   * payloads are transparently split into chunk frames and reassembled by
   * the peer. */
  async sendEnvelope(envelope: Envelope): Promise<void> {
    diagnostics.log("conn", `send ${envelope.type} encrypted=${!!this.sessionKey}`);
    if (this.sessionKey && this.peerDeviceId) {
      const enc = encrypt(this.sessionKey, this.peerDeviceId, envelope);
      for (const frame of splitEncrypted(enc)) {
        await this.transport.sendText(frame);
      }
    } else {
      await this.transport.sendText(serializeEnvelope(envelope));
    }
  }

  /** Send an envelope as plaintext regardless of the installed session key.
   * Used for heartbeats, which the relay must consume itself (an encrypted
   * frame would be routed to the peer and never counted for liveness). */
  async sendPlaintext(envelope: Envelope): Promise<void> {
    await this.transport.sendText(serializeEnvelope(envelope));
  }

  /** Receive the next envelope: decrypt an EncryptedEnvelope when a key is
   * installed, otherwise parse a plain Envelope. Returns null on clean close.
   * Chunked encrypted payloads are reassembled transparently. */
  async recvEnvelope(): Promise<Envelope | null> {
    for (;;) {
      const frame = await this.transport.recvText();
      if (frame === null) {
        console.log("[conn] recv closed");
        return null;
      }
      // Mixed framing: relay-originated frames (SubscriptionStatus acks,
      // pairing errors) are always plaintext — the relay holds no E2E key.
      // Route on shape instead of assuming everything is encrypted.
      if (!frame.includes('"to_device_id"')) {
        diagnostics.log("conn", "recv plain (relay-originated, key installed)");
        return parseEnvelope(frame);
      }
      const enc = JSON.parse(frame) as EncryptedEnvelope;
      let full = enc;
      if (enc.chunk_id !== undefined) {
        const reassembled = this.reassembleChunk(enc);
        if (reassembled === null) continue; // still waiting for more chunks
        full = reassembled;
      }
      if (!this.sessionKey) {
        diagnostics.log("conn", "recv encrypted before key installed; skipping");
        continue;
      }
      try {
        const env = decrypt(this.sessionKey, full);
        diagnostics.log("conn", `recv ${env.type} decrypted=ok`);
        return env;
      } catch (e) {
        diagnostics.log("conn", `recv decrypt FAILED: ${e}`);
        throw e;
      }
    }
  }

  /** Buffer a chunked fragment. Returns the fully reassembled envelope (chunk
   * metadata stripped) once every fragment of the same `chunk_id` arrives, or
   * null while still incomplete. */
  private reassembleChunk(enc: EncryptedEnvelope): EncryptedEnvelope | null {
    const chunkId = enc.chunk_id!;
    const index = enc.chunk_index ?? -1;
    const total = enc.chunk_total ?? 0;
    if (total <= 0 || index < 0 || index >= total) {
      this.chunks.delete(chunkId);
      return null;
    }
    let entry = this.chunks.get(chunkId);
    if (!entry) {
      entry = {
        total,
        received: Array.from({ length: total }, () => null),
        nonce: enc.nonce,
        toDeviceId: enc.to_device_id,
      };
      this.chunks.set(chunkId, entry);
    }
    if (entry.total !== total || entry.nonce.join(",") !== enc.nonce.join(",")) {
      this.chunks.delete(chunkId);
      return null;
    }
    entry.received[index] = new Uint8Array(enc.ciphertext);
    if (entry.received.some((part) => part === null)) return null;

    const ciphertext = new Uint8Array(
      entry.received.reduce((sum, part) => sum + (part?.length ?? 0), 0),
    );
    let offset = 0;
    for (const part of entry.received) {
      if (part) {
        ciphertext.set(part, offset);
        offset += part.length;
      }
    }
    const full: EncryptedEnvelope = {
      to_device_id: entry.toDeviceId,
      nonce: entry.nonce,
      ciphertext: Array.from(ciphertext),
    };
    this.chunks.delete(chunkId);
    return full;
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

/** Split an encrypted envelope into one or more serialized chunk frames. */
function splitEncrypted(enc: EncryptedEnvelope): string[] {
  const single = JSON.stringify(enc);
  if (single.length <= CHUNK_SINGLE_FRAME_BYTES) {
    return [single];
  }
  const chunkId = uuidV4();
  const pieces: Uint8Array[] = [];
  const bytes = new Uint8Array(enc.ciphertext);
  for (let i = 0; i < bytes.length; i += CHUNK_PAYLOAD_BYTES) {
    pieces.push(bytes.subarray(i, i + CHUNK_PAYLOAD_BYTES));
  }
  const total = pieces.length;
  return pieces.map((piece, index) =>
    JSON.stringify({
      to_device_id: enc.to_device_id,
      nonce: enc.nonce,
      ciphertext: Array.from(piece),
      chunk_id: chunkId,
      chunk_index: index,
      chunk_total: total,
    } as EncryptedEnvelope),
  );
}
// end of file
