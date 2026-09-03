import * as Haptics from "expo-haptics";
import type { HapticsPort } from "./presenter";

// expo-haptics adapter. Haptics can throw on devices without a haptic engine
// (and on web), so every call fails silently — an alert must never crash the
// receive loop.
export const expoHaptics: HapticsPort = {
  subtle: () => {
    // Medium (not Light): a light impact proved imperceptible on several
    // devices — the "watching" case must still confirm the turn ended.
    void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Medium).catch(() => {});
  },
  success: () => {
    void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success).catch(() => {});
  },
  warning: () => {
    void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Warning).catch(() => {});
  },
};
