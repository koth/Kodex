import { describe, it, expect } from "vitest";
import { RelayConnection } from "../relay/connection";
import { runReceiveLoop } from "../relay/driver";
import { linkedPair } from "./mock-relay";
import { fromMessage } from "../relay/framing";
import { ControlClient } from "../session/control-client";
import { deriveSessionKey } from "../crypto";

// `peer_session_reset` recovery path: when one side cannot decrypt the
// peer's traffic (stale/absent session key — e.g. the PC restarted and lost
// its in-memory key), it must learn immediately instead of waiting out a
// control-request timeout. The failing side emits an advisory plaintext
// reset; the receiver ends its receive loop so the reconnect ladder can run
// a fresh pairing_resume.

const KEY_A = deriveSessionKey(new Uint8Array(32).fill(1));
const KEY_B = deriveSessionKey(new Uint8Array(32).fill(2));

describe("peer_session_reset", () => {
  it("decrypt failure emits a best-effort peer_session_reset frame", async () => {
    const [phoneT, pcT] = linkedPair();
    const phone = new RelayConnection(phoneT, 30_000);
    const pc = new RelayConnection(pcT, 30_000);
    // Phone holds key A; PC encrypts with key B (simulates key rotation /
    // PC restart) so phone-side decryption fails.
    phone.installSessionKey(KEY_A, "pc-device-id");
    pc.installSessionKey(KEY_B, "phone-device-id");

    await pc.sendEnvelope(
      fromMessage("11111111-2222-4333-8444-555555555555", {
        type: "control_response",
        payload: { op: "cancel", request_id: "x" },
      }),
    );
    await expect(phone.recvEnvelope()).rejects.toThrow();

    // The advisory reset must land on the wire (plaintext, no to_device_id).
    const raw = await pcT.recvText();
    expect(raw).toContain('"peer_session_reset"');
    expect(raw).not.toContain('"to_device_id"');
  });

  it("reset notices are rate-limited per connection", async () => {
    const [phoneT, pcT] = linkedPair();
    const phone = new RelayConnection(phoneT, 30_000);
    const pc = new RelayConnection(pcT, 30_000);
    phone.installSessionKey(KEY_A, "pc-device-id");
    pc.installSessionKey(KEY_B, "phone-device-id");

    for (let i = 0; i < 3; i++) {
      await pc.sendEnvelope(fromMessage(null, { type: "event", payload: { kind: "snapshot_patch", patch: {} } }));
      await expect(phone.recvEnvelope()).rejects.toThrow();
      // Drain any emitted reset so the next iteration reads a data frame.
      void (await Promise.race([pcT.recvText(), Promise.resolve(null)]));
    }
    // Cooldown (30s) means only the FIRST failure produced a reset; after
    // draining one, nothing else may be queued.
    const next = await Promise.race([
      pcT.recvText(),
      new Promise<null>((r) => setTimeout(() => r(null), 50)),
    ]);
    expect(next).toBeNull();
  });

  it("receive loop ends ('closed') when the peer sends peer_session_reset", async () => {
    const [phoneT, pcT] = linkedPair();
    const phone = new RelayConnection(phoneT, 30_000);
    const pc = new RelayConnection(pcT, 30_000);
    const control = new ControlClient(phone as never);
    let stopped = false;
    const loop = runReceiveLoop(
      phone,
      control,
      () => {},
      () => {},
      () => stopped,
    );

    await pc.sendPlaintext(
      fromMessage(null, { type: "peer_session_reset", payload: {} }),
    );
    await expect(loop).resolves.toBe("closed");
  });

  it("ControlClient.failAll rejects pending requests immediately", async () => {
    const [phoneT] = linkedPair();
    const phone = new RelayConnection(phoneT, 30_000);
    const control = new ControlClient(phone as never);
    const pending = control.send({
      op: "cancel",
      request_id: "22222222-3333-4333-8333-222222222222",
    });
    control.failAll("connection lost");
    await expect(pending).rejects.toThrow("connection lost");
  });
});
