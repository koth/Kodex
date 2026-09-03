import { describe, it, expect } from "vitest";
import { SessionStore } from "../session/store";
import {
  TurnCompletionWatcher,
  INTERRUPTION_SUPPRESS_WINDOW_MS,
  type AlertPresenter,
  type TurnCompletionContext,
  type TurnOutcome,
} from "../session/turn-completion";
import type { UiSnapshot, SessionStatus } from "../types";

// Spec: mobile-turn-completion-alerts — Turn completion detection and
// User-initiated cancel suppression.

function makeSnapshot(sessionId: string, status: SessionStatus, title = "Demo", revision = 1): UiSnapshot {
  return {
    revision,
    workspace: { id: "ws-1", name: "demo", root: "/demo" },
    workspace_connected: true,
    session: {
      id: sessionId,
      workspace_id: "ws-1",
      title,
      model: "m",
      mode: null,
      agent_cli: null,
      status,
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
  };
}

function makeHarness() {
  const store = new SessionStore();
  const events: { outcome: TurnOutcome; ctx: TurnCompletionContext }[] = [];
  let nowMs = 1_000;
  let revision = 0;
  const presenter: AlertPresenter = {
    onTurnCompleted: (ctx) => events.push({ outcome: "completed", ctx }),
    onTurnInterrupted: (ctx) => events.push({ outcome: "interrupted", ctx }),
  };
  const watcher = new TurnCompletionWatcher(store, presenter, () => nowMs);
  return {
    store,
    events,
    watcher,
    advance: (ms: number) => {
      nowMs += ms;
    },
    // SessionStore drops same-session snapshots with a stale revision, so
    // each pushed state bumps it.
    setStatus: (sessionId: string, status: SessionStatus, title?: string) =>
      store.setSnapshot(makeSnapshot(sessionId, status, title, ++revision)),
  };
}

describe("TurnCompletionWatcher", () => {
  it("fires exactly once when a turn completes", () => {
    const h = makeHarness();
    h.setStatus("s-1", "Streaming");
    h.setStatus("s-1", "Idle");
    expect(h.events).toHaveLength(1);
    expect(h.events[0].outcome).toBe("completed");
    expect(h.events[0].ctx.sessionId).toBe("s-1");
    expect(h.events[0].ctx.sessionTitle).toBe("Demo");
  });

  it("does not double-fire when patch and status frame carry the same terminal state", () => {
    const h = makeHarness();
    h.setStatus("s-1", "Streaming");
    h.setStatus("s-1", "Idle"); // snapshot_patch path
    h.setStatus("s-1", "Idle"); // session_status_changed path, same state
    expect(h.events).toHaveLength(1);
  });

  it("does not fire for an initial/resynced snapshot that is already Idle", () => {
    const h = makeHarness();
    h.setStatus("s-1", "Idle");
    expect(h.events).toHaveLength(0);
  });

  it("does not fire on intra-turn flapping between active states", () => {
    const h = makeHarness();
    h.setStatus("s-1", "Streaming");
    h.setStatus("s-1", "WaitingForTool");
    h.setStatus("s-1", "Streaming");
    expect(h.events).toHaveLength(0);
  });

  it("distinguishes interruption from completion", () => {
    const h = makeHarness();
    h.setStatus("s-1", "WaitingForTool");
    h.setStatus("s-1", "Interrupted");
    expect(h.events).toHaveLength(1);
    expect(h.events[0].outcome).toBe("interrupted");
  });

  it("does not fire on session switch (baseline resets)", () => {
    const h = makeHarness();
    h.setStatus("s-1", "Streaming");
    h.store.clear(); // switch away wipes the store
    h.setStatus("s-2", "Idle"); // s-2's first snapshot is a baseline, not a turn end
    expect(h.events).toHaveLength(0);
  });

  it("suppresses a phone-initiated cancel within the window", () => {
    const h = makeHarness();
    h.setStatus("s-1", "Streaming");
    h.watcher.suppressNextInterruption();
    h.setStatus("s-1", "Interrupted");
    expect(h.events).toHaveLength(0);
  });

  it("suppression is one-shot: the next interruption alerts again", () => {
    const h = makeHarness();
    h.setStatus("s-1", "Streaming");
    h.watcher.suppressNextInterruption();
    h.setStatus("s-1", "Interrupted"); // suppressed
    h.setStatus("s-1", "Streaming");
    h.setStatus("s-1", "Interrupted"); // must alert
    expect(h.events).toHaveLength(1);
    expect(h.events[0].outcome).toBe("interrupted");
  });

  it("suppression expires after the window", () => {
    const h = makeHarness();
    h.watcher.suppressNextInterruption();
    h.advance(INTERRUPTION_SUPPRESS_WINDOW_MS + 1);
    h.setStatus("s-1", "Streaming");
    h.setStatus("s-1", "Interrupted");
    expect(h.events).toHaveLength(1);
  });

  it("records the turn start time for context", () => {
    const h = makeHarness();
    h.setStatus("s-1", "Idle");
    h.setStatus("s-1", "Streaming"); // turn starts at nowMs
    const start = 1_000; // nowMs at harness creation
    h.advance(3_000);
    h.setStatus("s-1", "Idle");
    expect(h.events[0].ctx.turnStartedAtMs).toBe(start);
  });
});
