import { describe, expect, it } from "vitest";
import {
  initialCompanionState,
  transition,
  moodAutoSettles,
  MOOD_SETTLE_MS,
} from "./companionStateMachine";
import type { CompanionEvent } from "./types";

const T0 = 1_000_000;

function run(events: Array<[CompanionEvent, number]>) {
  let state = initialCompanionState(T0);
  const results = [];
  for (const [event, now] of events) {
    const result = transition(state, event, now);
    results.push(result);
    state = result.state;
  }
  return { state, results };
}

describe("companionStateMachine", () => {
  it("prompt 开始 → thinking 并弹气泡", () => {
    const { state, results } = run([[{ kind: "prompt_started" }, T0 + 100]]);
    expect(state.mood).toBe("thinking");
    expect(results[0].showBubble).toBe(true);
  });

  it("权限请求 → awaiting_permission", () => {
    const { state } = run([
      [{ kind: "prompt_started" }, T0 + 100],
      [{ kind: "permission_requested" }, T0 + 200],
    ]);
    expect(state.mood).toBe("awaiting_permission");
  });

  it("工具运行 → working 但不弹气泡", () => {
    const { state, results } = run([
      [{ kind: "prompt_started" }, T0 + 100],
      [{ kind: "tool_running", toolName: "shell" }, T0 + 200],
      [{ kind: "tool_running", toolName: "read" }, T0 + 300],
    ]);
    expect(state.mood).toBe("working");
    expect(results[1].showBubble).toBe(false);
    // 同 mood 重复事件不产生气泡
    expect(results[2].showBubble).toBe(false);
  });

  it("完成 → happy 并弹气泡", () => {
    const { state, results } = run([[{ kind: "prompt_completed" }, T0 + 100]]);
    expect(state.mood).toBe("happy");
    expect(results[0].showBubble).toBe(true);
  });

  it("失败 → frustrated；取消 → pouty", () => {
    const failed = run([[{ kind: "prompt_failed", error: "boom" }, T0 + 100]]);
    expect(failed.state.mood).toBe("frustrated");
    const cancelled = run([[{ kind: "prompt_cancelled" }, T0 + 100]]);
    expect(cancelled.state.mood).toBe("pouty");
  });

  it("happy/frustrated/pouty 可自动回落 idle", () => {
    for (const mood of ["happy", "frustrated", "pouty", "curious"] as const) {
      expect(moodAutoSettles(mood)).toBe(true);
    }
    expect(moodAutoSettles("working")).toBe(false);
    const { state } = run([
      [{ kind: "prompt_completed" }, T0 + 100],
      [{ kind: "mood_settled" }, T0 + 100 + MOOD_SETTLE_MS],
    ]);
    expect(state.mood).toBe("idle");
  });

  it("空闲超时 → sleepy，且 mood_settled 无法唤醒", () => {
    const { state, results } = run([
      [{ kind: "idle_timeout" }, T0 + 100],
      [{ kind: "mood_settled" }, T0 + 200],
    ]);
    expect(state.mood).toBe("sleepy");
    expect(results[1].showBubble).toBe(false);
  });

  it("sleepy 可被会话事件唤醒", () => {
    const { state } = run([
      [{ kind: "idle_timeout" }, T0 + 100],
      [{ kind: "prompt_started" }, T0 + 200],
    ]);
    expect(state.mood).toBe("thinking");
  });

  it("用户交互不打断进行中的会话反馈", () => {
    const { state, results } = run([
      [{ kind: "prompt_started" }, T0 + 100],
      [{ kind: "user_interaction" }, T0 + 200],
    ]);
    expect(state.mood).toBe("thinking");
    expect(results[1].showBubble).toBe(false);
  });

  it("用户交互可打断 idle/sleepy → curious", () => {
    const { state } = run([
      [{ kind: "idle_timeout" }, T0 + 100],
      [{ kind: "user_interaction" }, T0 + 200],
    ]);
    expect(state.mood).toBe("curious");
  });
});
