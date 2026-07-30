import type { UiSnapshot, ToolInvocation, SessionStatus } from "../../../types";
import type { CompanionEvent } from "./types";

export const IDLE_TIMEOUT_MS = 5 * 60 * 1000;
/** 同类事件 3s 内不重复触发（spec: 气泡节流） */
export const EVENT_DEBOUNCE_MS = 3_000;

interface BridgeState {
  lastSessionStatus: SessionStatus | null;
  /** 最近一次会话状态变化的时刻（用于去抖窗口锚定） */
  statusChangedAt: number;
  lastEventKind: CompanionEvent["kind"] | null;
  lastEventAt: number;
  lastActivityAt: number;
  permissionPending: boolean;
}

export function createBridgeState(now: number): BridgeState {
  return {
    lastSessionStatus: null,
    statusChangedAt: 0,
    lastEventKind: null,
    lastEventAt: 0,
    lastActivityAt: now,
    permissionPending: false,
  };
}

function hasPendingPermission(tools: ToolInvocation[]): boolean {
  return tools.some(
    (tool) =>
      !tool.permission_decision &&
      (tool.permission_options.length > 0 ||
        (tool.permission_input?.questions.length ?? 0) > 0),
  );
}

function hasRunningTool(tools: ToolInvocation[]): boolean {
  return tools.some((tool) => tool.status === "Running" || tool.status === "Pending");
}

/**
 * 纯函数：UiSnapshot 增量 → CompanionEvent（可单测）。
 * 返回 null 表示无需驱动状态机。
 */
export function mapSnapshot(
  state: BridgeState,
  snapshot: UiSnapshot,
  now: number,
): { state: BridgeState; event: CompanionEvent | null } {
  const status = snapshot.session.status;
  const previous = state.lastSessionStatus;
  if (status !== previous) {
    state.statusChangedAt = now;
  }
  let event: CompanionEvent | null = null;

  if (status === "Interrupted") {
    if (previous !== "Interrupted") {
      event = { kind: "prompt_cancelled" };
    }
  } else if (status === "Streaming" || status === "WaitingForTool") {
    if (previous === "Idle" || previous === "Interrupted" || previous === null) {
      event = { kind: "prompt_started" };
    }
    const permissionPending = hasPendingPermission(snapshot.tools);
    if (permissionPending && !state.permissionPending) {
      event = { kind: "permission_requested" };
    }
    state.permissionPending = permissionPending;
    if (event === null && hasRunningTool(snapshot.tools)) {
      // 高频工具事件只驱动动画（状态机侧 SILENT_MOODS 不弹气泡）
      const running = snapshot.tools.find((tool) => tool.status === "Running");
      event = { kind: "tool_running", toolName: running?.name ?? "" };
    }
  } else if (status === "Idle" && (previous === "Streaming" || previous === "WaitingForTool")) {
    const failed = snapshot.tools.some((tool) => tool.status === "Failed");
    event = failed ? { kind: "prompt_failed" } : { kind: "prompt_completed" };
    state.permissionPending = false;
  }

  if (event !== null) {
    state.lastActivityAt = now;
    // 同类事件 3s 内不重复触发（spec: 气泡节流）。
    // 「同类」指状态机侧的持续性动作：tool_running / permission_requested
    // 会在持续状态下反复映射，必须节流；而 prompt_started/completed/failed/
    // cancelled 是状态迁移的瞬时事件，天然不重复，不参与节流窗口。
    const throttled = event.kind === "tool_running" || event.kind === "permission_requested";
    if (throttled && event.kind === state.lastEventKind && now - state.lastEventAt < EVENT_DEBOUNCE_MS) {
      event = null;
    } else {
      state.lastEventKind = event.kind;
      state.lastEventAt = now;
    }
  }
  state.lastSessionStatus = status;
  return { state, event };
}

/** 空闲超时判定：无会话活动且无用户交互 */
export function checkIdleTimeout(state: BridgeState, now: number): CompanionEvent | null {
  if (now - state.lastActivityAt >= IDLE_TIMEOUT_MS) {
    state.lastActivityAt = now;
    return { kind: "idle_timeout" };
  }
  return null;
}

export function noteUserInteraction(state: BridgeState, now: number): CompanionEvent {
  state.lastActivityAt = now;
  return { kind: "user_interaction" };
}
