import { describe, expect, it } from "vitest";
import {
  createBridgeState,
  mapSnapshot,
  checkIdleTimeout,
  noteUserInteraction,
  EVENT_DEBOUNCE_MS,
  IDLE_TIMEOUT_MS,
} from "./companionBridge";
import type { UiSnapshot, ToolInvocation, SessionStatus } from "../../../types";

const T0 = 1_000_000;

function tool(partial: Partial<ToolInvocation>): ToolInvocation {
  return {
    id: "t1",
    call_id: "c1",
    parent_call_id: null,
    name: "shell",
    kind: "execute",
    summary: "",
    status: "Running",
    is_subagent: false,
    detail_text: "",
    logs: [],
    diff_paths: [],
    diff_previews: [],
    raw_input: null,
    raw_output: null,
    terminal_output: null,
    error: null,
    permission_options: [],
    permission_input: null,
    permission_decision: null,
    can_stop: false,
    stop_kind: null,
    stop_status: null,
    ...partial,
  };
}

function snapshot(status: SessionStatus, tools: ToolInvocation[] = []): UiSnapshot {
  return { session: { status }, tools } as unknown as UiSnapshot;
}

describe("companionBridge", () => {
  it("Idle → Streaming 触发 prompt_started", () => {
    let state = createBridgeState(T0);
    let result = mapSnapshot(state, snapshot("Idle"), T0);
    state = result.state;
    result = mapSnapshot(state, snapshot("Streaming"), T0 + 100);
    expect(result.event?.kind).toBe("prompt_started");
  });

  it("等待权限触发 permission_requested（仅一次）", () => {
    let state = createBridgeState(T0);
    let result = mapSnapshot(state, snapshot("Streaming"), T0);
    state = result.state;
    const permTool = tool({ permission_options: [{ id: "allow", label: "允许" }] as never });
    result = mapSnapshot(state, snapshot("WaitingForTool", [permTool]), T0 + 100);
    expect(result.event?.kind).toBe("permission_requested");
    state = result.state;
    result = mapSnapshot(state, snapshot("WaitingForTool", [permTool]), T0 + 200);
    expect(result.event?.kind).not.toBe("permission_requested");
  });

  it("持续性事件 3s 去抖（tool_running 连续映射只发一次）", () => {
    let state = createBridgeState(T0);
    let result = mapSnapshot(state, snapshot("Streaming"), T0);
    state = result.state;
    const running = [tool({ status: "Running" })];
    result = mapSnapshot(state, snapshot("WaitingForTool", running), T0 + 100);
    expect(result.event?.kind).toBe("tool_running");
    state = result.state;
    // 3s 内继续 Running → 被去抖
    result = mapSnapshot(state, snapshot("WaitingForTool", running), T0 + 200);
    expect(result.event).toBeNull();
    // 超过去抖窗口 → 再次触发
    result = mapSnapshot(state, snapshot("WaitingForTool", running), T0 + EVENT_DEBOUNCE_MS + 200);
    expect(result.event?.kind).toBe("tool_running");
  });

  it("瞬时事件（prompt_started/completed）不参与节流", () => {
    let state = createBridgeState(T0);
    let result = mapSnapshot(state, snapshot("Streaming"), T0);
    expect(result.event?.kind).toBe("prompt_started");
    state = result.state;
    result = mapSnapshot(state, snapshot("Idle"), T0 + 100);
    expect(result.event?.kind).toBe("prompt_completed");
    state = result.state;
    // 紧接着新一轮：prompt_started 立即可见，不受 3s 窗口影响
    result = mapSnapshot(state, snapshot("Streaming"), T0 + 200);
    expect(result.event?.kind).toBe("prompt_started");
    state = result.state;
    result = mapSnapshot(state, snapshot("Idle"), T0 + 300);
    expect(result.event?.kind).toBe("prompt_completed");
  });

  it("工具失败 → prompt_failed；正常 → prompt_completed", () => {
    let state = createBridgeState(T0);
    let result = mapSnapshot(state, snapshot("Streaming"), T0);
    state = result.state;
    result = mapSnapshot(state, snapshot("Idle", [tool({ status: "Failed" })]), T0 + 100);
    expect(result.event?.kind).toBe("prompt_failed");

    state = createBridgeState(T0);
    result = mapSnapshot(state, snapshot("Streaming"), T0);
    state = result.state;
    result = mapSnapshot(state, snapshot("Idle", [tool({ status: "Succeeded" })]), T0 + 100);
    expect(result.event?.kind).toBe("prompt_completed");
  });

  it("Interrupted → prompt_cancelled", () => {
    let state = createBridgeState(T0);
    let result = mapSnapshot(state, snapshot("Streaming"), T0);
    state = result.state;
    result = mapSnapshot(state, snapshot("Interrupted"), T0 + 100);
    expect(result.event?.kind).toBe("prompt_cancelled");
  });

  it("空闲超时触发 idle_timeout，用户交互重置计时", () => {
    const state = createBridgeState(T0);
    expect(checkIdleTimeout(state, T0 + IDLE_TIMEOUT_MS - 1)).toBeNull();
    expect(checkIdleTimeout(state, T0 + IDLE_TIMEOUT_MS + 1)?.kind).toBe("idle_timeout");
    noteUserInteraction(state, T0 + IDLE_TIMEOUT_MS + 2);
    expect(checkIdleTimeout(state, T0 + IDLE_TIMEOUT_MS * 2)).toBeNull();
  });
});
