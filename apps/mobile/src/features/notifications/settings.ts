import type { SecretStore } from "../../crypto/identity";

// Turn-completion alert preferences (spec: mobile-turn-completion-alerts —
// Alert settings and persistence). Stored as JSON in the SecretStore
// (Keychain/Keystore) so they survive app restarts without introducing
// another storage dependency. All reads merge over the defaults, so a
// settings blob written by an older app version can never produce an
// undefined field.

export interface AlertSettings {
  /** Master switch: off suppresses every alert channel. */
  enabled: boolean;
  /** Play the completion/interruption chime (foreground) / notification sound. */
  sound: boolean;
  /** Haptic feedback (foreground) / notification vibration. */
  vibration: boolean;
  /** Only alert when the app is backgrounded; foreground stays silent. */
  backgroundOnly: boolean;
  /** Post a system local notification when the app is backgrounded. */
  systemNotifications: boolean;
}

export const DEFAULT_ALERT_SETTINGS: AlertSettings = {
  enabled: true,
  sound: true,
  vibration: true,
  backgroundOnly: false,
  systemNotifications: true,
};

const STORAGE_KEY = "alert-settings-v1";

export async function loadAlertSettings(store: SecretStore): Promise<AlertSettings> {
  const raw = await store.get(STORAGE_KEY);
  if (!raw) return { ...DEFAULT_ALERT_SETTINGS };
  try {
    const parsed = JSON.parse(new TextDecoder().decode(raw)) as Partial<AlertSettings>;
    return { ...DEFAULT_ALERT_SETTINGS, ...parsed };
  } catch {
    return { ...DEFAULT_ALERT_SETTINGS };
  }
}

export async function saveAlertSettings(
  store: SecretStore,
  settings: AlertSettings,
): Promise<void> {
  await store.set(STORAGE_KEY, new TextEncoder().encode(JSON.stringify(settings)));
}
