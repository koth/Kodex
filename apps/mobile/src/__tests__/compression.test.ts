import { describe, it, expect } from "vitest";
import { encrypt, decrypt, ENCODING_GZIP, NONCE_LEN } from "../crypto/aead";
import type { Envelope } from "../types/relay-protocol";

// Payload-compression behavior of the AEAD framing: large envelopes are
// gzip'd BEFORE encryption and flagged via EncryptedEnvelope.encoding so the
// receiver inverts it after decrypt. Byte-aligned with relay-client::crypto
// (flate2) — the snapshot flood that killed mobile WebSockets shipped a
// ~450KB snapshot as 5 chunk frames; compressed it fits one frame.

const KEY = { bytes: Uint8Array.from({ length: 32 }, (_, i) => 7 * i + 1) };
const PEER = "pc-device-id";

function bigEnvelope(): Envelope {
  return {
    proto_version: 1,
    id: null,
    type: "event",
    payload: {
      kind: "snapshot_full",
      snapshot: {
        filler: "x".repeat(64 * 1024),
        lines: Array.from({ length: 512 }, (_, i) => `line-${i}-aaaaaaaaaaaaaaaaaaaa`),
      },
    },
  };
}

describe("aead payload compression", () => {
  it("large payloads are gzipped (encoding=gzip) and roundtrip", () => {
    const envelope = bigEnvelope();
    const rawLen = JSON.stringify(envelope).length;
    const encrypted = encrypt(KEY, PEER, envelope);
    expect(encrypted.encoding).toBe(ENCODING_GZIP);
    // Sanity: this repetitive payload must actually shrink.
    expect(encrypted.ciphertext.length).toBeLessThan(rawLen / 2);
    expect(decrypt(KEY, encrypted)).toEqual(envelope);
  });

  it("small payloads stay raw (no encoding field)", () => {
    const envelope: Envelope = {
      proto_version: 1,
      id: null,
      type: "control_request",
      payload: { op: "cancel", request_id: "r-1" },
    };
    const encrypted = encrypt(KEY, PEER, envelope);
    expect(encrypted.encoding).toBeUndefined();
    expect("encoding" in encrypted).toBe(false);
    expect(decrypt(KEY, encrypted)).toEqual(envelope);
  });

  it("gzip flag survives the chunk split/reassemble round trip", async () => {
    // Drive the real connection layer: an oversized gzipped envelope is sent
    // as multiple chunk frames and reassembled on the peer before decrypt.
    const { RelayConnection } = await import("../relay/connection");
    const { linkedPair } = await import("./mock-relay");
    const [phoneT, pcT] = linkedPair();
    const phone = new RelayConnection(phoneT, 30_000);
    const pc = new RelayConnection(pcT, 30_000);
    phone.installSessionKey(KEY, PEER);
    pc.installSessionKey(KEY, "phone-device-id");

    await phone.sendEnvelope(bigEnvelope());
    const got = await pc.recvEnvelope();
    expect(got?.type).toBe("event");
    expect((got?.payload as { kind?: string }).kind).toBe("snapshot_full");
  });
});

describe("aead basics still hold", () => {
  it("nonce length stays 12", () => {
    expect(NONCE_LEN).toBe(12);
  });
});
