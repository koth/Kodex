import { describe, it, expect } from "vitest";
import {
  DEFAULT_ALERT_SETTINGS,
  loadAlertSettings,
  saveAlertSettings,
} from "../features/notifications/settings";
import { InMemorySecretStore } from "../util/in-memory-store";

// Spec: mobile-turn-completion-alerts — Alert settings and persistence.

describe("alert settings persistence", () => {
  it("returns defaults when nothing is stored", async () => {
    const store = new InMemorySecretStore();
    expect(await loadAlertSettings(store)).toEqual(DEFAULT_ALERT_SETTINGS);
  });

  it("round-trips settings across a simulated restart", async () => {
    const store = new InMemorySecretStore();
    const settings = { ...DEFAULT_ALERT_SETTINGS, sound: false, backgroundOnly: true };
    await saveAlertSettings(store, settings);
    // A fresh load from the same store is what a cold restart does.
    expect(await loadAlertSettings(store)).toEqual(settings);
  });

  it("merges partial/old blobs over the defaults", async () => {
    const store = new InMemorySecretStore();
    await store.set(
      "alert-settings-v1",
      new TextEncoder().encode(JSON.stringify({ enabled: false })),
    );
    const loaded = await loadAlertSettings(store);
    expect(loaded.enabled).toBe(false);
    expect(loaded.sound).toBe(DEFAULT_ALERT_SETTINGS.sound);
    expect(loaded.systemNotifications).toBe(DEFAULT_ALERT_SETTINGS.systemNotifications);
  });

  it("falls back to defaults on a corrupted blob", async () => {
    const store = new InMemorySecretStore();
    await store.set("alert-settings-v1", new TextEncoder().encode("not json{"));
    expect(await loadAlertSettings(store)).toEqual(DEFAULT_ALERT_SETTINGS);
  });
});
