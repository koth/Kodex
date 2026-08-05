import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, renderHook } from "@testing-library/react";
import type { UiSnapshot } from "../../types";
import { sessionLoadHistoryBefore } from "../../lib/tauri";
import { useWorkbenchSnapshot } from "./useWorkbenchSnapshot";

vi.mock("../../lib/events", () => ({
  onUiSnapshot: vi.fn(() => Promise.resolve(() => {})),
  onUiSnapshotPatch: vi.fn(() => Promise.resolve(() => {})),
}));

vi.mock("../../lib/tauri", () => ({
  startupPerfMark: vi.fn(() => Promise.resolve()),
  sessionGetState: vi.fn(() => Promise.reject(new Error("no mock"))),
  sessionGetRevision: vi.fn(() => Promise.reject(new Error("no mock"))),
  sessionLoadHistoryBefore: vi.fn(() => Promise.reject(new Error("no mock"))),
}));

const mockedLoadHistoryBefore = vi.mocked(sessionLoadHistoryBefore);

function makeSnapshot(overrides: Partial<UiSnapshot> = {}): UiSnapshot {
  return {
    revision: 1,
    workspace: { id: "ws-1", name: "test", root: "/test" },
    session: {
      id: "s-1",
      workspace_id: "ws-1",
      title: "test",
      model: "test-model",
      mode: null,
      agent_cli: null,
      status: "Idle",
    },
    session_config: { hydrated: false, controls: [] },
    prompt_capabilities: { image: false, embedded_context: false, session_steer: false },
    available_commands: [],
    agent_plan: [],
    messages: [],
    timeline: [],
    tools: [],
    repository: { branch: "main", head: "abc", changed_files: [] },
    inspector_tab: "Activity",
    inspector_sections: [],
    session_changes: [],
    review_changes: [],
    turn_changes: [],
    thinking_status: null,
    ...overrides,
  };
}

describe("useWorkbenchSnapshot – loadOlderHistory", () => {
  beforeEach(() => {
    mockedLoadHistoryBefore.mockReset();
  });

  it("returns false without paging when there is no older history", async () => {
    const snapshot = makeSnapshot({ history_earliest_seq: null });
    const { result } = renderHook(() => useWorkbenchSnapshot());
    act(() => {
      result.current.acceptSnapshot(snapshot);
    });

    let loaded: boolean | undefined;
    await act(async () => {
      loaded = await result.current.loadOlderHistory();
    });

    expect(loaded).toBe(false);
    expect(mockedLoadHistoryBefore).not.toHaveBeenCalled();
  });

  it("prepends the older page, dedupes overlap, and advances the cursor", async () => {
    const snapshot = makeSnapshot({
      messages: [{ id: "m2", role: "User", body: "newer" }],
      timeline: [{ Message: "m2" }],
      history_total: 3,
      history_earliest_seq: 20,
    });
    mockedLoadHistoryBefore.mockResolvedValue({
      messages: [
        { id: "m1", role: "User", body: "older" },
        // Overlapping entry must be deduped, not duplicated.
        { id: "m2", role: "User", body: "newer" },
      ],
      tools: [],
      timeline: [{ Message: "m1" }, { Message: "m2" }],
      earliest_seq: 10,
      has_more: true,
    });
    const { result } = renderHook(() => useWorkbenchSnapshot());
    act(() => {
      result.current.acceptSnapshot(snapshot);
    });

    let loaded: boolean | undefined;
    await act(async () => {
      loaded = await result.current.loadOlderHistory(200);
    });

    expect(loaded).toBe(true);
    expect(mockedLoadHistoryBefore).toHaveBeenCalledWith(20, 200);
    const next = result.current.snapshot;
    expect(next?.messages.map((message) => message.id)).toEqual(["m1", "m2"]);
    expect(next?.timeline).toEqual([{ Message: "m1" }, { Message: "m2" }]);
    expect(next?.history_earliest_seq).toBe(10);
  });

  it("clears the cursor when the page reports no more history", async () => {
    const snapshot = makeSnapshot({
      history_total: 2,
      history_earliest_seq: 5,
    });
    mockedLoadHistoryBefore.mockResolvedValue({
      messages: [{ id: "m0", role: "Assistant", body: "oldest" }],
      tools: [],
      timeline: [{ Message: "m0" }],
      earliest_seq: 1,
      has_more: false,
    });
    const { result } = renderHook(() => useWorkbenchSnapshot());
    act(() => {
      result.current.acceptSnapshot(snapshot);
    });

    await act(async () => {
      await result.current.loadOlderHistory();
    });

    expect(result.current.snapshot?.history_earliest_seq).toBeNull();
    // With the cursor cleared, further calls must not hit the backend again.
    let loaded: boolean | undefined;
    await act(async () => {
      loaded = await result.current.loadOlderHistory();
    });
    expect(loaded).toBe(false);
    expect(mockedLoadHistoryBefore).toHaveBeenCalledTimes(1);
  });

  it("keeps the current snapshot when paging fails", async () => {
    const snapshot = makeSnapshot({ history_earliest_seq: 20 });
    mockedLoadHistoryBefore.mockRejectedValue(new Error("boom"));
    const { result } = renderHook(() => useWorkbenchSnapshot());
    act(() => {
      result.current.acceptSnapshot(snapshot);
    });

    let loaded: boolean | undefined;
    await act(async () => {
      loaded = await result.current.loadOlderHistory();
    });

    expect(loaded).toBe(false);
    expect(result.current.snapshot?.history_earliest_seq).toBe(20);
  });
});
