import type { CompanionSettings } from "./types";
import { DEFAULT_COMPANION_SETTINGS } from "./types";

const STORAGE_KEY = "kodex.companionSettings";
/** 同窗口内设置变更通知（localStorage 的 storage 事件只跨窗口触发） */
export const COMPANION_SETTINGS_EVENT = "companion:settings-changed";

export function loadCompanionSettings(): CompanionSettings {
  if (typeof window === "undefined") return { ...DEFAULT_COMPANION_SETTINGS };
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return { ...DEFAULT_COMPANION_SETTINGS };
    const parsed = JSON.parse(raw) as Partial<CompanionSettings>;
    return {
      ...DEFAULT_COMPANION_SETTINGS,
      ...parsed,
      position: {
        ...DEFAULT_COMPANION_SETTINGS.position,
        ...(parsed.position ?? {}),
      },
    };
  } catch {
    return { ...DEFAULT_COMPANION_SETTINGS };
  }
}

export function saveCompanionSettings(settings: CompanionSettings): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(settings));
    window.dispatchEvent(new CustomEvent(COMPANION_SETTINGS_EVENT));
  } catch {
    // 存储失败静默忽略（与既有 useLeftSidebarState 等模式一致）
  }
}

/** 订阅设置变更（同窗口 CustomEvent + 跨窗口 storage 事件） */
export function onCompanionSettingsChanged(callback: (settings: CompanionSettings) => void): () => void {
  const handleCustom = () => callback(loadCompanionSettings());
  const handleStorage = (event: StorageEvent) => {
    if (event.key === STORAGE_KEY) callback(loadCompanionSettings());
  };
  window.addEventListener(COMPANION_SETTINGS_EVENT, handleCustom);
  window.addEventListener("storage", handleStorage);
  return () => {
    window.removeEventListener(COMPANION_SETTINGS_EVENT, handleCustom);
    window.removeEventListener("storage", handleStorage);
  };
}
