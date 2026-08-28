import { describe, it, expect } from "vitest";
import {
  BOUND_DEVICE_KEY,
  BOUND_DEVICES_KEY,
  loadBoundDevices,
  saveBoundDevices,
  upsertBoundDevice,
  removeBoundDevice,
  clearAllBoundDevices,
  bindOutcomeFromResponse,
  canReconnectWithoutRescan,
  type BoundDevice,
} from "../account/binding";
import { InMemorySecretStore } from "../util/in-memory-store";
import {
  subscriptionStateFromStatus,
  NO_SUBSCRIPTION,
  demoteOnExpiry,
  type SubscriptionState,
} from "../account/subscription";
import type { BindDeviceResponse, SubscriptionStatus } from "../types/relay-protocol";

// Multi-machine binding persistence: every scanned QR yields its own pairing
// token, so the phone stores a LIST of BoundDevice (keyed by peer_device_id).
// The legacy single-record key migrates into the list on first load.

function makeBound(peer = "pc-dev"): BoundDevice {
  return {
    device_id: "phone-dev",
    auth_token: "tok-abc",
    pairing_token: `ptok-${peer}`,
    peer_device_id: peer,
    peer_static_pubkey_b64: "pc-x25519",
  };
}

describe("BoundDevice multi-machine persistence", () => {
  it("stores and reloads several machines under the list key", async () => {
    const store = new InMemorySecretStore();
    await upsertBoundDevice(store, makeBound("pc-a"));
    await upsertBoundDevice(store, makeBound("pc-b"));
    const loaded = await loadBoundDevices(store);
    expect(loaded.map((d) => d.peer_device_id).sort()).toEqual(["pc-a", "pc-b"]);
    // The SessionKey is never persisted here: bindings hold only the account
    // + pairing tokens (the E2E key is re-derived per resume).
    const raw = JSON.parse(new TextDecoder().decode((await store.get(BOUND_DEVICES_KEY))!));
    expect(Array.isArray(raw)).toBe(true);
    expect("session_key" in raw[0]).toBe(false);
  });

  it("upserts by peer_device_id instead of duplicating a machine", async () => {
    const store = new InMemorySecretStore();
    await upsertBoundDevice(store, makeBound("pc-a"));
    const updated = { ...makeBound("pc-a"), pairing_token: "ptok-refreshed" };
    await upsertBoundDevice(store, updated);
    const loaded = await loadBoundDevices(store);
    expect(loaded).toHaveLength(1);
    expect(loaded[0].pairing_token).toBe("ptok-refreshed");
  });

  it("removes one machine while keeping the others", async () => {
    const store = new InMemorySecretStore();
    await upsertBoundDevice(store, makeBound("pc-a"));
    await upsertBoundDevice(store, makeBound("pc-b"));
    const remaining = await removeBoundDevice(store, "pc-a");
    expect(remaining.map((d) => d.peer_device_id)).toEqual(["pc-b"]);
    expect((await loadBoundDevices(store)).map((d) => d.peer_device_id)).toEqual(["pc-b"]);
  });

  it("clears every machine (and the legacy key) via clearAllBoundDevices", async () => {
    const store = new InMemorySecretStore();
    await upsertBoundDevice(store, makeBound("pc-a"));
    await clearAllBoundDevices(store);
    expect(await loadBoundDevices(store)).toEqual([]);
  });

  it("returns an empty list when nothing is stored", async () => {
    const store = new InMemorySecretStore();
    expect(await loadBoundDevices(store)).toEqual([]);
  });

  it("migrates the legacy single-record binding into the list on first load", async () => {
    const store = new InMemorySecretStore();
    const legacy = makeBound("pc-legacy");
    await store.set(BOUND_DEVICE_KEY, new TextEncoder().encode(JSON.stringify(legacy)));
    const loaded = await loadBoundDevices(store);
    expect(loaded).toEqual([legacy]);
    // Migration is durable: the legacy key is gone, the list carries the record.
    expect(await store.get(BOUND_DEVICE_KEY)).toBeNull();
    expect((await loadBoundDevices(store))).toEqual([legacy]);
    expect(await store.get(BOUND_DEVICES_KEY)).not.toBeNull();
  });

  it("drops a corrupt legacy record instead of crashing", async () => {
    const store = new InMemorySecretStore();
    await store.set(BOUND_DEVICE_KEY, new TextEncoder().encode("{not json"));
    expect(await loadBoundDevices(store)).toEqual([]);
    expect(await store.get(BOUND_DEVICE_KEY)).toBeNull();
  });

  it("drops corrupt list bytes and falls back to the legacy record", async () => {
    const store = new InMemorySecretStore();
    const legacy = makeBound("pc-legacy");
    await store.set(BOUND_DEVICES_KEY, new TextEncoder().encode("{broken"));
    await store.set(BOUND_DEVICE_KEY, new TextEncoder().encode(JSON.stringify(legacy)));
    expect(await loadBoundDevices(store)).toEqual([legacy]);
  });

  it("saveBoundDevices round-trips an explicit list", async () => {
    const store = new InMemorySecretStore();
    const list = [makeBound("pc-a"), makeBound("pc-b")];
    await saveBoundDevices(store, list);
    expect(await loadBoundDevices(store)).toEqual(list);
  });
});

describe("bindOutcomeFromResponse", () => {
  const bound = makeBound();

  it("maps an ok=true response to a bound outcome", () => {
    const response: BindDeviceResponse = {
      ok: true,
      bound_device_id: "phone-dev",
    };
    const outcome = bindOutcomeFromResponse(
      response,
      bound.auth_token,
      bound.pairing_token,
      bound.peer_device_id,
      bound.peer_static_pubkey_b64,
    );
    expect(outcome.kind).toBe("bound");
    if (outcome.kind === "bound") {
      expect(outcome.bound).toEqual(bound);
    }
  });

  it("maps a subscription-rejection to subscription_required", () => {
    const response: BindDeviceResponse = {
      ok: false,
      bound_device_id: "",
      message: "no active subscription",
    };
    const outcome = bindOutcomeFromResponse(
      response,
      bound.auth_token,
      bound.pairing_token,
      bound.peer_device_id,
      bound.peer_static_pubkey_b64,
    );
    expect(outcome.kind).toBe("subscription_required");
  });

  it("maps any other rejection to a failed outcome with the message", () => {
    const response: BindDeviceResponse = {
      ok: false,
      bound_device_id: "",
      message: "device limit reached",
    };
    const outcome = bindOutcomeFromResponse(
      response,
      bound.auth_token,
      bound.pairing_token,
      bound.peer_device_id,
      bound.peer_static_pubkey_b64,
    );
    expect(outcome.kind).toBe("failed");
    if (outcome.kind === "failed") {
      expect(outcome.message).toBe("device limit reached");
    }
  });

  it("falls back to a generic message when the relay omits one", () => {
    const response: BindDeviceResponse = {
      ok: false,
      bound_device_id: "",
    };
    const outcome = bindOutcomeFromResponse(
      response,
      bound.auth_token,
      bound.pairing_token,
      bound.peer_device_id,
      bound.peer_static_pubkey_b64,
    );
    expect(outcome.kind).toBe("failed");
    if (outcome.kind === "failed") {
      expect(outcome.message.length).toBeGreaterThan(0);
    }
  });
});

describe("canReconnectWithoutRescan", () => {
  const bound = makeBound();

  it("is true only when the subscription is active AND a binding exists", () => {
    expect(canReconnectWithoutRescan(true, bound)).toBe(true);
  });

  it("is false on the free tier (no binding) even if active", () => {
    expect(canReconnectWithoutRescan(true, null)).toBe(false);
  });

  it("is false when the subscription is inactive even if bound", () => {
    expect(canReconnectWithoutRescan(false, bound)).toBe(false);
  });

  it("is false for a free-tier unbound device", () => {
    expect(canReconnectWithoutRescan(false, null)).toBe(false);
  });
});

describe("subscription state", () => {
  it("maps a SubscriptionStatus to a client-side state", () => {
    const status: SubscriptionStatus = {
      active: true,
      plan: "pro",
      expires_at: 1_700_000_000_000,
    };
    expect(subscriptionStateFromStatus(status)).toEqual<SubscriptionState>({
      active: true,
      plan: "pro",
      expiresAt: 1_700_000_000_000,
    });
  });

  it("NO_SUBSCRIPTION is the inactive free-tier default", () => {
    expect(NO_SUBSCRIPTION).toEqual<SubscriptionState>({
      active: false,
      plan: null,
      expiresAt: null,
    });
  });

  it("demoteOnExpiry flags re-scan when an active subscription lapses", () => {
    const current: SubscriptionState = {
      active: true,
      plan: "pro",
      expiresAt: 1_700_000_000_000,
    };
    const pushed: SubscriptionStatus = { active: false };
    const { state, mustRescan } = demoteOnExpiry(current, pushed);
    expect(state.active).toBe(false);
    expect(mustRescan).toBe(true);
  });

  it("demoteOnExpiry does not flag re-scan on an inactive->inactive transition", () => {
    const { mustRescan } = demoteOnExpiry(NO_SUBSCRIPTION, {
      active: false,
    });
    expect(mustRescan).toBe(false);
  });

  it("demoteOnExpiry does not flag re-scan when a subscription activates", () => {
    const { mustRescan } = demoteOnExpiry(NO_SUBSCRIPTION, {
      active: true,
      plan: "pro",
    });
    expect(mustRescan).toBe(false);
  });
});
// end of file
