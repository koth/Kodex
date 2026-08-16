import type { SecretStore } from "../crypto/identity";
import type { SessionKey } from "../crypto/session-key";

export const SESSION_KEY_STORAGE_KEY = "kodex.active-session";

export interface PersistedSession {
  key: SessionKey;
  peer_device_id: string;
}

export async function persistSession(
  store: SecretStore,
  session: PersistedSession,
): Promise<void> {
  const json = JSON.stringify({
    key: Array.from(session.key.bytes),
    peer_device_id: session.peer_device_id,
  });
  await store.set(SESSION_KEY_STORAGE_KEY, new TextEncoder().encode(json));
}

export async function loadSession(
  store: SecretStore,
): Promise<PersistedSession | null> {
  const raw = await store.get(SESSION_KEY_STORAGE_KEY);
  if (!raw) return null;
  let value: { key: number[]; peer_device_id: string };
  try {
    value = JSON.parse(new TextDecoder().decode(raw));
  } catch {
    await store.delete(SESSION_KEY_STORAGE_KEY);
    return null;
  }
  const bytes = Uint8Array.from(value.key ?? []);
  if (bytes.length !== 32 || !value.peer_device_id) {
    await store.delete(SESSION_KEY_STORAGE_KEY);
    return null;
  }
  return {
    key: { bytes },
    peer_device_id: value.peer_device_id,
  };
}

export async function clearSession(store: SecretStore): Promise<void> {
  await store.delete(SESSION_KEY_STORAGE_KEY);
}
