import type { AppTheme } from "./types";

export const DEFAULT_APP_THEME: AppTheme = "graphite";

export interface AppThemeDefinition {
  id: AppTheme;
  label: string;
  description: string;
  swatches: string[];
}

export const APP_THEMES: AppThemeDefinition[] = [
  {
    id: "graphite",
    label: "深色",
    description: "冷静石墨底 + 钢蓝强调，适合长时间编码。",
    swatches: ["#0c0d0f", "#16181c", "#7aa2c7"],
  },
  {
    id: "light",
    label: "浅色",
    description: "明亮低噪的冷灰界面。",
    swatches: ["#f7f8fa", "#e9ecef", "#3f6f97"],
  },
];

const THEME_IDS = new Set<AppTheme>(APP_THEMES.map((theme) => theme.id));
const LEGACY_DARK_THEMES = new Set(["kodex_dark", "midnight", "forest"]);

export function resolveAppTheme(theme: string | null | undefined): AppTheme {
  if (LEGACY_DARK_THEMES.has(theme ?? "")) return "graphite";
  return THEME_IDS.has(theme as AppTheme) ? (theme as AppTheme) : DEFAULT_APP_THEME;
}

export function applyAppTheme(theme: string | null | undefined): AppTheme {
  const resolved = resolveAppTheme(theme);
  document.documentElement.dataset.theme = resolved;
  return resolved;
}

export function getAppliedAppTheme(): AppTheme {
  return resolveAppTheme(document.documentElement.dataset.theme);
}
