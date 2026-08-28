import type { EventFrame } from "../types/relay-protocol";
import type { UiSnapshot as Snapshot, PermissionInputRequest, UiSnapshotPatch } from "../types";
import {
  applySnapshotPatch,
  applyToolUpdated,
  applySessionStatus,
  materializeStreamingMessageBodies,
} from "./reducer";
import {
  appendStreamingMessageDelta,
  clearAllStreamingMessages,
} from "./streaming-message-store";

type Listener = (snapshot: Snapshot | null) => void;
type PermissionHandler = (request: PermissionInputRequest) => void;
type ResyncHandler = () => void;

// Single UiSnapshot per active session + EventFrame application. Mirrors the
// desktop useWorkbenchSnapshot reducer + guard:
// - a SnapshotPatch is ignored if its session.id differs from the held session
//   or its revision is lower (stale). Revisions are per-session.
// - delta-only streaming patches fold the live stream store back into message
//   bodies, so assistant text grows in real time during a turn.
// - a revision GAP means a patch frame was lost on the wire; applying further
//   patches would misalign the delta chain, so a full resync is requested
//   instead (the controller debounces it into one GetState).
// - a GetState response that raced behind a newer pushed snapshot never
//   regresses the held state (it would wedge subsequent patches).
export class SessionStore {
  private snapshot: Snapshot | null = null;
  private activeSessionId: string | null = null;
  private listeners = new Set<Listener>();
  private permissionHandler: PermissionHandler | null = null;
  private resyncHandler: ResyncHandler | null = null;

  subscribe(listener: Listener): () => void {
    this.listeners.add(listener);
    listener(this.snapshot);
    return () => this.listeners.delete(listener);
  }

  get state(): Snapshot | null {
    return this.snapshot;
  }

  setPermissionHandler(handler: PermissionHandler | null): void {
    this.permissionHandler = handler;
  }

  /** Set the callback invoked when a lost patch (revision gap) requires a
   * full re-sync. The controller debounces this into a single GetState. */
  setResyncHandler(handler: ResyncHandler | null): void {
    this.resyncHandler = handler;
  }

  private requestResync(): void {
    this.resyncHandler?.();
  }

  /** Replace the entire snapshot (GetState response / SnapshotFull). A
   * same-session snapshot OLDER than the held one is dropped: it raced
   * behind patches that already landed, and accepting it would rewind the
   * revision so every subsequent patch fails the freshness guard. */
  setSnapshot(snapshot: Snapshot): void {
    if (
      this.snapshot !== null &&
      snapshot.session.id === this.snapshot.session.id &&
      snapshot.revision <= this.snapshot.revision
    ) {
      return;
    }
    this.activeSessionId = snapshot.session.id;
    this.snapshot = materializeStreamingMessageBodies(snapshot);
    this.emit();
  }

  /** Clear local state (e.g. on unbind/session switch reset). */
  clear(): void {
    this.activeSessionId = null;
    this.snapshot = null;
    clearAllStreamingMessages();
    this.emit();
  }

  /** Reset local snapshot for an incoming session before its data arrives.
   * Guards later full-snapshot events so a stale previous session cannot
   * overwrite the newly-selected one. Streaming bodies from the previous
   * session are dropped too (they only ever grow in memory otherwise). */
  beginSession(sessionId: string): void {
    this.activeSessionId = sessionId;
    this.snapshot = null;
    clearAllStreamingMessages();
    this.emit();
  }

  /** Apply an inbound EventFrame with the stale/duplicate patch guard. */
  applyEventFrame(frame: EventFrame): void {
    switch (frame.kind) {
      case "snapshot_full": {
        const incoming = frame.snapshot as Snapshot;
        if (this.activeSessionId && incoming.session.id !== this.activeSessionId) {
          break;
        }
        this.activeSessionId = incoming.session.id;
        this.snapshot = materializeStreamingMessageBodies(incoming);
      }
      break;
      case "snapshot_patch": {
        if (!this.snapshot) break;
        const patch = frame.patch as unknown as UiSnapshotPatch;
        if (patch.session.id !== this.snapshot.session.id) break;
        if (patch.revision < this.snapshot.revision) break; // stale
        const hasDeltas = (patch.message_deltas?.length ?? 0) > 0;
        if (patch.revision === this.snapshot.revision && !hasDeltas) break; // duplicate
        // A revision gap means a patch frame was lost: the cursor-based
        // diffs that follow are relative to a state we never saw. Ask for a
        // full snapshot instead of merging a misaligned chain.
        if (patch.revision > this.snapshot.revision + 1) {
          this.requestResync();
          break;
        }
        // Delta-only patches (empty `messages`) carry the growing assistant
        // text as `message_deltas`: land them in the streaming store (the
        // first delta is seeded with the snapshot body, mirroring the
        // desktop's render-time `ensureStreamingMessageBody`), apply the rest
        // of the patch (tools/timeline/status), then fold the live stream
        // bodies back in so the timeline renders the growing text instead of
        // a truncated prefix.
        const messageBodies = new Map(
          this.snapshot.messages.map((m) => [m.id, m.body] as const),
        );
        for (const delta of patch.message_deltas ?? []) {
          appendStreamingMessageDelta(
            delta.id,
            delta.append,
            messageBodies.get(delta.id),
          );
        }
        this.snapshot = materializeStreamingMessageBodies(
          applySnapshotPatch(this.snapshot, patch),
        );
        break;
      }
      case "tool_updated":
        if (this.snapshot) this.snapshot = applyToolUpdated(this.snapshot, frame.tool as unknown as import("../types").ToolInvocation);
        break;
      case "session_status_changed":
        if (this.snapshot) {
          this.snapshot = applySessionStatus(
            this.snapshot,
            frame.status as Snapshot["session"]["status"],
          );
        }
        break;
      case "permission_request":
        if (this.permissionHandler) this.permissionHandler(frame.request);
        break;
    }
    this.emit();
  }

  private emit(): void {
    for (const l of this.listeners) l(this.snapshot);
  }
}
// end of file