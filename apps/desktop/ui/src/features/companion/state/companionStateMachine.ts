import type { CompanionEvent, CompanionMood, CompanionState } from "./types";

/** happy/pouty 等情绪停留上限（ms），超时回到 idle */
export const MOOD_SETTLE_MS = 10_000;

/** 高频事件只更新动画、不弹气泡 */
const SILENT_MOODS: ReadonlySet<CompanionMood> = new Set(["working", "curious"]);

const TRANSITIONS: Record<CompanionEvent["kind"], CompanionMood> = {
  prompt_started: "thinking",
  tool_running: "working",
  permission_requested: "awaiting_permission",
  prompt_completed: "happy",
  prompt_failed: "frustrated",
  prompt_cancelled: "pouty",
  idle_timeout: "sleepy",
  user_interaction: "curious",
  mood_settled: "idle",
};

/** 用户交互只会打断低强度状态，不会打断进行中的会话反馈 */
const INTERACTION_INTERRUPTIBLE: ReadonlySet<CompanionMood> = new Set([
  "idle",
  "sleepy",
  "curious",
  "happy",
  "pouty",
]);

/** sleepy 只能被会话事件或用户交互唤醒 */
function wakeAllowed(event: CompanionEvent): boolean {
  return event.kind !== "mood_settled";
}

export interface TransitionResult {
  state: CompanionState;
  /** 本次迁移是否应触发气泡 */
  showBubble: boolean;
}

export function initialCompanionState(now: number): CompanionState {
  return { mood: "idle", bubble: null, enteredAt: now };
}

/**
 * 纯函数状态迁移。气泡规则：
 * - working / curious 迁移不弹气泡（SILENT_MOODS）
 * - mood 未变化时不弹气泡（节流由 bridge 的 3s 去抖配合）
 */
export function transition(
  state: CompanionState,
  event: CompanionEvent,
  now: number,
): TransitionResult {
  if (state.mood === "sleepy" && !wakeAllowed(event)) {
    return { state, showBubble: false };
  }
  if (
    event.kind === "user_interaction" &&
    !INTERACTION_INTERRUPTIBLE.has(state.mood)
  ) {
    return { state, showBubble: false };
  }

  const nextMood: CompanionMood = TRANSITIONS[event.kind];
  if (nextMood === state.mood) {
    return { state, showBubble: false };
  }

  const showBubble = !SILENT_MOODS.has(nextMood);
  return {
    state: { mood: nextMood, bubble: null, enteredAt: now },
    showBubble,
  };
}

/** 情绪是否需要在 MOOD_SETTLE_MS 后自动回落到 idle */
export function moodAutoSettles(mood: CompanionMood): boolean {
  return mood === "happy" || mood === "frustrated" || mood === "pouty" || mood === "curious";
}
