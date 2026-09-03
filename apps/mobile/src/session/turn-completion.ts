import type { SessionStore } from "./store";
import type { SessionStatus } from "../types";
import { diagnostics } from "../util/diagnostics";

// Turn-completion detection (spec: mobile-turn-completion-alerts).
//
// The watcher subscribes to the SessionStore and diffs consecutive snapshots'
// `session.id` + `session.status`. A turn ends when the status leaves an
// ACTIVE state for a terminal one: `Idle` = completed, `Interrupted` =
// interrupted/failed. Because the diff runs on the emitted snapshot, it
// covers every write path (`snapshot_patch` wholesale session replacement and
// `session_status_changed` frames alike) and is naturally idempotent: two
// frames carrying the same terminal state are one transition, one alert.
//
// No alert without a baseline: the first snapshot seen for a session (initial
// load, reconnect full-sync, session switch) only establishes the baseline —
// opening an already-idle session must not chime.
//
// The watcher is pure logic: presentation is delegated to an injected
// AlertPresenter so unit tests never touch native modules.

export type TurnOutcome = "completed" | "interrupted";

export interface TurnCompletionContext {
  sessionId: string;
  sessionTitle: string;
  /** When the turn entered an active state, if observed; null otherwise. */
  turnStartedAtMs: number | null;
}

export interface AlertPresenter {
  onTurnCompleted(ctx: TurnCompletionContext): void;
  onTurnInterrupted(ctx: TurnCompletionContext): void;
}

const ACTIVE_STATUSES: ReadonlySet<SessionStatus> = new Set(["Streaming", "WaitingForTool"]);

/** One-shot window for `suppressNextInterruption` (phone-initiated cancel). */
export const INTERRUPTION_SUPPRESS_WINDOW_MS = 5000;

export class TurnCompletionWatcher {
  private baselineSessionId: string | null = null;
  private baselineStatus: SessionStatus | null = null;
  private turnStartedAtMs: number | null = null;
  private suppressInterruptedUntilMs = 0;
  private readonly unsubscribe: () => void;

  constructor(
    store: SessionStore,
    private readonly presenter: AlertPresenter,
    private readonly now: () => number = () => Date.now(),
  ) {
    this.unsubscribe = store.subscribe((snapshot) => this.onSnapshot(snapshot));
  }

  /** Suppress the interruption alert caused by the user's own cancel. */
  suppressNextInterruption(): void {
    this.suppressInterruptedUntilMs = this.now() + INTERRUPTION_SUPPRESS_WINDOW_MS;
  }

  dispose(): void {
    this.unsubscribe();
  }

  private onSnapshot(snapshot: import("../types").UiSnapshot | null): void {
    if (!snapshot) {
      // Session switch / disconnect wiped the store: the next snapshot is a
      // fresh baseline, never an alert.
      this.baselineSessionId = null;
      this.baselineStatus = null;
      this.turnStartedAtMs = null;
      return;
    }
    const { id, status, title } = snapshot.session;
    if (id !== this.baselineSessionId) {
      this.baselineSessionId = id;
      this.baselineStatus = status;
      this.turnStartedAtMs = ACTIVE_STATUSES.has(status) ? this.now() : null;
      return;
    }
    const prev = this.baselineStatus;
    this.baselineStatus = status;
    if (prev === null || prev === status) return;

    const wasActive = ACTIVE_STATUSES.has(prev);
    const isActive = ACTIVE_STATUSES.has(status);
    if (isActive) {
      // Entering (or moving within) an active state: no alert, just track
      // when the turn started.
      if (!wasActive) this.turnStartedAtMs = this.now();
      return;
    }
    if (!wasActive) return; // Idle -> Interrupted etc.: no turn was observed.

    const ctx: TurnCompletionContext = {
      sessionId: id,
      sessionTitle: title,
      turnStartedAtMs: this.turnStartedAtMs,
    };
    this.turnStartedAtMs = null;

    if (status === "Idle") {
      diagnostics.log("alerts", `turn completed: session=${id.slice(0, 8)}`);
      this.presenter.onTurnCompleted(ctx);
      return;
    }
    if (status === "Interrupted") {
      if (this.now() < this.suppressInterruptedUntilMs) {
        diagnostics.log("alerts", "interruption alert suppressed (user-initiated cancel)");
        this.suppressInterruptedUntilMs = 0; // one-shot
        return;
      }
      diagnostics.log("alerts", `turn interrupted: session=${id.slice(0, 8)}`);
      this.presenter.onTurnInterrupted(ctx);
    }
  }
}
