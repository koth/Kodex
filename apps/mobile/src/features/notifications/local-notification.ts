import { Platform } from "react-native";
import * as Notifications from "expo-notifications";
import { diagnostics } from "../../util/diagnostics";
import type { TurnCompletionContext, TurnOutcome } from "../../session/turn-completion";
import type { NotifyPort } from "./presenter";

// System local notifications (spec: mobile-turn-completion-alerts — System
// notification channel and permission).
//
// Android channel caveat: channel sound/vibration are owned by the OS after
// creation and cannot be changed, while the user CAN toggle sound/vibration
// in settings. The two toggles therefore map to four pre-registered channels.
// The channel sound is always the completion chime; interruption is
// distinguished by the body text (doubling channels per sound is not worth
// it).
//
// Reachability limit: a local notification can only fire while the app
// process and its relay connection are alive. A killed app cannot alert —
// that needs server-side push (FCM/APNs), which is a separate change.

const CHANNEL_FULL = "turn-complete"; // sound + vibration
const CHANNEL_NO_SOUND = "turn-complete-nosound"; // vibration only
const CHANNEL_NO_VIBRATION = "turn-complete-novibrate"; // sound only
const CHANNEL_QUIET = "turn-complete-quiet"; // neither

const VIBRATION_PATTERN = [0, 250, 250, 250];

function channelFor(opts: { sound: boolean; vibration: boolean }): string {
  if (opts.sound && opts.vibration) return CHANNEL_FULL;
  if (opts.sound) return CHANNEL_NO_VIBRATION;
  if (opts.vibration) return CHANNEL_NO_SOUND;
  return CHANNEL_QUIET;
}

let setupDone = false;

/** Register the foreground handler + Android channels. Idempotent. */
export async function ensureNotificationSetup(): Promise<void> {
  if (setupDone) return;
  setupDone = true;
  // Foreground presentation is owned by the in-app banner (spec: no duplicate
  // system banner while the app is foregrounded), so the handler suppresses
  // everything; background delivery is unaffected by the handler.
  Notifications.setNotificationHandler({
    handleNotification: async () => ({
      shouldShowBanner: false,
      shouldShowList: false,
      shouldPlaySound: false,
      shouldSetBadge: false,
    }),
  });
  if (Platform.OS !== "android") return;
  await Notifications.setNotificationChannelAsync(CHANNEL_FULL, {
    name: "任务完成提醒",
    importance: Notifications.AndroidImportance.HIGH,
    sound: "turn_complete.wav",
    vibrationPattern: VIBRATION_PATTERN,
  });
  await Notifications.setNotificationChannelAsync(CHANNEL_NO_SOUND, {
    name: "任务完成提醒（静音）",
    importance: Notifications.AndroidImportance.HIGH,
    vibrationPattern: VIBRATION_PATTERN,
  });
  await Notifications.setNotificationChannelAsync(CHANNEL_NO_VIBRATION, {
    name: "任务完成提醒（无震动）",
    importance: Notifications.AndroidImportance.HIGH,
    sound: "turn_complete.wav",
  });
  await Notifications.setNotificationChannelAsync(CHANNEL_QUIET, {
    name: "任务完成提醒（静默）",
    importance: Notifications.AndroidImportance.DEFAULT,
  });
}

export type NotificationPermissionState = "granted" | "denied" | "undetermined";

export async function getNotificationPermissionState(): Promise<NotificationPermissionState> {
  try {
    const { status } = await Notifications.getPermissionsAsync();
    if (status === Notifications.PermissionStatus.GRANTED) return "granted";
    if (status === Notifications.PermissionStatus.DENIED) return "denied";
    return "undetermined";
  } catch {
    return "denied";
  }
}

/** Request the runtime permission (Android 13+ / iOS). Returns granted?. */
export async function requestNotificationPermission(): Promise<boolean> {
  try {
    const { status } = await Notifications.requestPermissionsAsync();
    return status === Notifications.PermissionStatus.GRANTED;
  } catch {
    return false;
  }
}

function copy(outcome: TurnOutcome, title: string): { title: string; body: string } {
  const session = title.trim() || "会话";
  return outcome === "completed"
    ? { title: "任务已完成 ✅", body: `「${session}」本轮已完成` }
    : { title: "任务已中断 ⚠️", body: `「${session}」本轮被中断` };
}

export const localNotifier: NotifyPort = {
  notify(ctx, outcome, opts) {
    void (async () => {
      try {
        const permission = await getNotificationPermissionState();
        if (permission !== "granted") {
          // The single most common "no notification at all" cause — make it
          // visible in the diagnostics screen instead of failing silently.
          diagnostics.log("alerts", `notify skipped: notification permission=${permission}`);
          return;
        }
        await ensureNotificationSetup();
        const { title, body } = copy(outcome, ctx.sessionTitle);
        await Notifications.scheduleNotificationAsync({
          content: {
            title,
            body,
            // iOS honours the per-notification sound; Android 8+ uses the
            // channel's sound instead.
            sound: opts.sound ? "turn_complete.wav" : undefined,
            ...(Platform.OS === "android"
              ? { channelId: channelFor(opts) }
              : null),
            data: { sessionId: ctx.sessionId },
          },
          trigger: null, // immediate
        });
        diagnostics.log("alerts", `system notification posted (${outcome})`);
      } catch (e) {
        diagnostics.log(
          "alerts",
          `notify failed: ${e instanceof Error ? e.message : String(e)}`,
        );
      }
    })();
  },
};
