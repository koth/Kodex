import { Platform, StyleSheet } from "react-native";

// Premium dark-first design system for the Maju companion app.
//
// The palette leans into a deep, slightly cool near-black with layered
// elevated surfaces, a vivid indigo-blue accent, and tinted status chips.
// Depth comes from soft shadows + a 1px hairline highlight on raised surfaces
// rather than flat fills, so the chrome reads "high-end" without any extra
// native dependencies (gradients/emulators are simulated with stacked Views).
export const colors = {
  // Base canvas: a touch of blue-violet so pure black never looks flat.
  bg: "#07080f",
  // Raised layers, each a clear step up in luminance.
  surface: "#11131f",
  surfaceAlt: "#171a28",
  surfaceRaised: "#1d2132",
  // Hairlines: faint cool border + a brighter "top highlight" for edges.
  border: "#232838",
  borderStrong: "#2f3650",
  // Text.
  text: "#eef1fb",
  textDim: "#8b93ab",
  textFaint: "#5b6275",
  // Accent: vivid indigo-blue with a brighter mate for gradient fills.
  accent: "#5b8cff",
  accentBright: "#8aa9ff",
  accentDim: "#1b2a63",
  accentTint: "rgba(91,140,255,0.14)",
  // Semantic colors + their soft tinted backgrounds for chips.
  success: "#34d399",
  successTint: "rgba(52,211,153,0.14)",
  danger: "#fb7185",
  dangerTint: "rgba(251,113,133,0.14)",
  warn: "#fbbf24",
  warnTint: "rgba(251,191,36,0.14)",
  mono: "#07080f",
  // Overlay scrim for sheets.
  scrim: "rgba(4,5,12,0.72)",
} as const;

export const radius = { sm: 8, md: 12, lg: 16, xl: 22, pill: 999 } as const;

export const spacing = { xs: 4, sm: 8, md: 12, lg: 16, xl: 24, xxl: 32 } as const;

// Elevation presets. iOS honors shadow*; Android gets a subtle elevation plus a
// faux top-highlight border so raised surfaces pop on both platforms.
export const shadows = {
  // Subtle resting elevation for cards.
  card: Platform.select({
    ios: { shadowColor: "#000", shadowOpacity: 0.32, shadowRadius: 14, shadowOffset: { width: 0, height: 6 } },
    android: { elevation: 3 },
    default: {},
  }) as object,
  // Floating sheets / headers.
  raised: Platform.select({
    ios: { shadowColor: "#000", shadowOpacity: 0.45, shadowRadius: 22, shadowOffset: { width: 0, height: 10 } },
    android: { elevation: 8 },
    default: {},
  }) as object,
  // Pressed/active accent surface.
  glow: Platform.select({
    ios: { shadowColor: colors.accent, shadowOpacity: 0.35, shadowRadius: 16, shadowOffset: { width: 0, height: 6 } },
    android: { elevation: 6 },
    default: {},
  }) as object,
} as const;

// Shared RN styles for the companion app. Existing keys are preserved for
// backward compatibility but refined; new premium primitives (chips, avatars,
// hairlines, hero text) are added for the redesign.
export const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: colors.bg },
  center: { flex: 1, backgroundColor: colors.bg, alignItems: "center", justifyContent: "center", padding: spacing.xl },
  card: { backgroundColor: colors.surface, borderRadius: radius.lg, padding: spacing.lg, marginVertical: spacing.sm, marginHorizontal: spacing.sm, borderWidth: 1, borderColor: colors.border, ...shadows.card },
  title: { color: colors.text, fontSize: 26, fontWeight: "800", letterSpacing: -0.4, marginBottom: spacing.sm },
  subtitle: { color: colors.textDim, fontSize: 14, lineHeight: 20, marginBottom: spacing.lg },
  row: { flexDirection: "row", alignItems: "center" },
  rowBetween: { flexDirection: "row", alignItems: "center", justifyContent: "space-between" },
  text: { color: colors.text, fontSize: 15 },
  textDim: { color: colors.textDim, fontSize: 13 },
  textFaint: { color: colors.textFaint, fontSize: 12 },
  mono: { color: colors.text, fontFamily: "monospace", fontSize: 12 },
  input: { color: colors.text, backgroundColor: colors.surfaceAlt, borderRadius: radius.md, padding: spacing.md, fontSize: 15, borderWidth: 1, borderColor: colors.border, minHeight: 46 },
  button: { backgroundColor: colors.accent, borderRadius: radius.md, paddingVertical: spacing.md + 2, paddingHorizontal: spacing.lg, alignItems: "center", justifyContent: "center", ...shadows.glow },
  buttonDanger: { backgroundColor: colors.danger, borderRadius: radius.md, paddingVertical: spacing.md + 2, paddingHorizontal: spacing.lg, alignItems: "center", justifyContent: "center" },
  buttonGhost: { borderRadius: radius.md, paddingVertical: spacing.md, paddingHorizontal: spacing.lg, alignItems: "center", justifyContent: "center", borderWidth: 1, borderColor: colors.borderStrong, backgroundColor: colors.surface },
  buttonText: { color: "#fff", fontSize: 15, fontWeight: "700", letterSpacing: 0.1 },
  status: { fontSize: 12, color: colors.textDim, marginLeft: spacing.xs },
  badge: { paddingHorizontal: spacing.sm + 2, paddingVertical: spacing.xs + 1, borderRadius: radius.pill, backgroundColor: colors.surfaceAlt, borderWidth: 1, borderColor: colors.border },
  sectionHeader: { color: colors.textFaint, fontSize: 11, fontWeight: "700", letterSpacing: 0.8, textTransform: "uppercase", marginHorizontal: spacing.lg, marginTop: spacing.lg, marginBottom: spacing.xs },
  // New: hairline divider.
  hairline: { height: StyleSheet.hairlineWidth, backgroundColor: colors.border },
  // New: small tinted status chip (tint via inline style color override).
  chip: { flexDirection: "row", alignItems: "center", paddingHorizontal: spacing.sm + 2, paddingVertical: 3, borderRadius: radius.pill, borderWidth: 1 },
  // New: square/round avatar container; pass bgColor + text inline.
  avatar: { alignItems: "center", justifyContent: "center", borderRadius: radius.md },
  avatarText: { color: "#fff", fontWeight: "800", fontSize: 15, letterSpacing: 0.2 },
  // New: pill-shaped primary action (used in composer / hero).
  pillButton: { backgroundColor: colors.accent, borderRadius: radius.pill, alignItems: "center", justifyContent: "center", ...shadows.glow },
});
// end of file
