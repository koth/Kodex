import { useCallback, useEffect, useRef, useState } from "react";
import type { UiSnapshot, UiSnapshotPatch } from "../../types";
import { startupPerfMark, sessionGetState, sessionGetRevision, sessionLoadHistoryBefore } from "../../lib/tauri";
import { onUiSnapshot, onUiSnapshotPatch } from "../../lib/events";
import {
  appendStreamingMessageDelta,
  clearStreamingMessageBodies,
  flushStreamingMessageBodies,
  getStreamingMessageBody,
} from "../conversation/streaming-message-store";

/** How often the workbench re-syncs its snapshot from the backend. Streaming
 *  renders depend on incremental `ui:snapshot_patch` events whose deltas are
 *  only correct when applied in-order with no gaps. A single dropped event
 *  (webview busy under heavy markdown rendering) silently desyncs the local
 *  stream store from the backend message body, truncating the final reply
 *  with no way to recover. This periodic full poll is the self-heal: when the
 *  backend revision is ahead of ours we replace the local snapshot with the
 *  complete state, so a missed delta self-corrects within a few seconds. */
const SNAPSHOT_SELF_HEAL_POLL_MS = 3000;

export function applySnapshotPatch(snapshot: UiSnapshot, patch: UiSnapshotPatch): UiSnapshot {
  const messages =
    patch.messages.length === 0
      ? snapshot.messages
      : mergeMessagesById(snapshot.messages, patch.messages);
  const tools =
    patch.tools.length === 0
      ? snapshot.tools
      : mergeById(snapshot.tools, patch.tools);
  const timeline =
    patch.timeline.length === 0 && patch.timeline_start === snapshot.timeline.length
      ? snapshot.timeline
      : [...snapshot.timeline.slice(0, patch.timeline_start), ...patch.timeline];

  return {
    ...snapshot,
    revision: patch.revision,
    session: patch.session,
    session_config: patch.session_config,
    prompt_capabilities: patch.prompt_capabilities,
    available_commands: patch.available_commands,
    agent_plan: patch.agent_plan,
    messages,
    timeline,
    tools,
    repository: patch.repository ?? snapshot.repository,
    inspector_tab: patch.inspector_tab,
    inspector_sections: patch.inspector_sections,
    session_changes: patch.session_changes,
    review_changes: patch.review_changes,
    turn_changes: patch.turn_changes ?? snapshot.turn_changes ?? [],
    thinking_status: patch.thinking_status,
    thinking_text: patch.thinking_text ?? snapshot.thinking_text,
    // The backend always sends the full replacement list of pending steers
    // (empty once they have been moved into the timeline).
    pending_steers: patch.pending_steers ?? snapshot.pending_steers ?? [],
  };
}

function mergeMessagesById(
  current: UiSnapshot["messages"],
  updates: UiSnapshot["messages"],
): UiSnapshot["messages"] {
  if (updates.length === 0) return current;
  const next = current.slice();
  const appended: UiSnapshot["messages"] = [];

  for (const update of updates) {
    const index = next.findIndex((item) => item.id === update.id);
    if (index >= 0) {
      const currentMessage = next[index];
      const shouldKeepLongerCurrentBody =
        currentMessage.role === update.role &&
        currentMessage.role === "Assistant" &&
        currentMessage.body.length > update.body.length &&
        currentMessage.body.startsWith(update.body);
      const nextMessage = shouldKeepLongerCurrentBody
        ? { ...update, body: currentMessage.body }
        : update;
      if (next[index] !== nextMessage) {
        next[index] = nextMessage;
      }
    } else {
      appended.push(update);
    }
  }

  return appended.length === 0 ? next : [...next, ...appended];
}

function mergeById<T extends { id: string }>(current: T[], updates: T[]): T[] {
  if (updates.length === 0) return current;
  const next = current.slice();
  const appended: T[] = [];

  for (const update of updates) {
    const index = next.findIndex((item) => item.id === update.id);
    if (index >= 0) {
      if (next[index] !== update) {
        next[index] = update;
      }
    } else {
      appended.push(update);
    }
  }

  return appended.length === 0 ? next : [...next, ...appended];
}

function applyStreamingDeltas(patch: UiSnapshotPatch) {
  for (const delta of patch.message_deltas ?? []) {
    appendStreamingMessageDelta(delta.id, delta.append);
  }
}

function isStreamingDeltaOnlyPatch(patch: UiSnapshotPatch) {
  return (
    patch.session.status === "Streaming" &&
    (patch.message_deltas?.length ?? 0) > 0 &&
    patch.messages.length === 0 &&
    patch.timeline.length === 0 &&
    patch.tools.length === 0 &&
    patch.repository == null
  );
}

export function materializeStreamingMessageBodies(snapshot: UiSnapshot): UiSnapshot {
  // Pending stream flushes are debounced; force them out before we decide whether
  // the snapshot body is stale relative to the live stream store.
  flushStreamingMessageBodies();
  let changed = false;
  const messages = snapshot.messages.map((message) => {
    const streamingBody = getStreamingMessageBody(message.id);
    if (
      streamingBody == null ||
      streamingBody === message.body ||
      streamingBody.length <= message.body.length
    ) {
      return message;
    }
    // Prefer the longer stream body whenever it is a continuation OR the
    // snapshot body is only a stale prefix-incompatible fragment. The common
    // failure mode is delta-only patches updating the stream store while
    // `snapshot.messages` stays truncated; when streaming ends the UI would
    // otherwise render the truncated snapshot body.
    const streamIsContinuation = streamingBody.startsWith(message.body);
    const snapshotLooksStalePrefix =
      message.role === "Assistant" &&
      message.body.length > 0 &&
      streamingBody.includes(message.body);
    if (!streamIsContinuation && !snapshotLooksStalePrefix) {
      return message;
    }
    changed = true;
    return { ...message, body: streamingBody };
  });
  return changed ? { ...snapshot, messages } : snapshot;
}

export function useWorkbenchSnapshot() {
  const [snapshot, setSnapshot] = useState<UiSnapshot | null>(null);
  const [workspaceReady, setWorkspaceReady] = useState(false);
  // Track BOTH session id and revision. Revision is per-session (starts at 1,
  // bumps by 1), so two sessions can share the same revision value. Without
  // the session-id guard a stale event from the previous session (same
  // revision number) can block the new session's snapshot from being
  // accepted after a switch.
  const prevSnapshotRevision = useRef<number>(0);
  const prevSnapshotSessionId = useRef<string>("");
  const snapshotRef = useRef<UiSnapshot | null>(null);
  const firstSnapshotLogged = useRef(false);
  const firstWorkspaceReadyLogged = useRef(false);

  useEffect(() => {
    snapshotRef.current = snapshot;
    if (snapshot && !firstSnapshotLogged.current) {
      firstSnapshotLogged.current = true;
      void startupPerfMark(
        "workbench/first_snapshot_committed",
        `revision=${snapshot.revision} messages=${snapshot.messages.length} tools=${snapshot.tools.length} timeline=${snapshot.timeline.length}`,
      );
      requestAnimationFrame(() => {
        void startupPerfMark(
          "workbench/first_snapshot_painted",
          `performance_now=${performance.now().toFixed(1)}`,
        );
      });
    }
  }, [snapshot]);

  // Free cached streaming bodies when the session changes — the stream store
  // only ever grows otherwise, so long sessions accumulate every historical
  // message body in memory.
  const currentSessionId = snapshot?.session.id ?? null;
  useEffect(() => {
    return () => {
      clearStreamingMessageBodies();
    };
  }, [currentSessionId]);

  const pollState = useCallback(async () => {
    try {
      const state = await sessionGetState();
      if (
        state.session.id !== prevSnapshotSessionId.current ||
        state.revision !== prevSnapshotRevision.current
      ) {
        prevSnapshotSessionId.current = state.session.id;
        prevSnapshotRevision.current = state.revision;
        setSnapshot(materializeStreamingMessageBodies(state));
      }
    } catch {
      // No workspace open; the welcome screen remains the source of truth.
    }
  }, []);

  const acceptSnapshot = useCallback((nextSnapshot: UiSnapshot) => {
    prevSnapshotSessionId.current = nextSnapshot.session.id;
    prevSnapshotRevision.current = nextSnapshot.revision;
    setWorkspaceReady(true);
    setSnapshot(materializeStreamingMessageBodies(nextSnapshot));
  }, []);

  const clearSnapshot = useCallback(() => {
    prevSnapshotSessionId.current = "";
    prevSnapshotRevision.current = 0;
    setSnapshot(null);
  }, []);

  const clearWorkspace = useCallback(() => {
    prevSnapshotSessionId.current = "";
    prevSnapshotRevision.current = 0;
    setWorkspaceReady(false);
    setSnapshot(null);
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    let unlistenPatch: (() => void) | undefined;
    // Debounce full-snapshot re-syncs triggered by patch reconcile paths so a
    // burst of gap/stale patches during streaming doesn't fire several full
    // `session_get_state` clones back-to-back.
    let reconcileTimer = 0;
    const scheduleFullResync = () => {
      if (reconcileTimer !== 0) return;
      reconcileTimer = window.setTimeout(() => {
        reconcileTimer = 0;
        void pollState();
      }, 120);
    };

    onUiSnapshot((nextSnapshot) => {
      if (disposed) return;
      if (
        nextSnapshot.session.id === prevSnapshotSessionId.current &&
        nextSnapshot.revision === prevSnapshotRevision.current
      )
        return;
      prevSnapshotSessionId.current = nextSnapshot.session.id;
      prevSnapshotRevision.current = nextSnapshot.revision;
      setWorkspaceReady(true);
      if (!firstWorkspaceReadyLogged.current) {
        firstWorkspaceReadyLogged.current = true;
        void startupPerfMark(
          "workbench/ui_snapshot_event_first",
          `revision=${nextSnapshot.revision} messages=${nextSnapshot.messages.length} tools=${nextSnapshot.tools.length} timeline=${nextSnapshot.timeline.length}`,
        );
      }
      setSnapshot(materializeStreamingMessageBodies(nextSnapshot));
    })
      .then((cleanup) => {
        if (disposed) {
          cleanup();
          return;
        }
        unlisten = cleanup;
      })
      .catch(() => {});

    onUiSnapshotPatch((patch) => {
      if (disposed) return;
      const hasDeltas = (patch.message_deltas?.length ?? 0) > 0;
      const isDuplicateRevision =
        patch.session.id === prevSnapshotSessionId.current &&
        patch.revision === prevSnapshotRevision.current;
      // Same-revision patches are normally ignored, but streaming deltas must
      // still land in the stream store + snapshot bodies.
      if (isDuplicateRevision && !hasDeltas) return;

      if (hasDeltas) {
        applyStreamingDeltas(patch);
      }
      setWorkspaceReady(true);
      setSnapshot((prev) => {
        if (!prev) {
          scheduleFullResync();
          return prev;
        }
        // Reject stale patches that belong to a different session than the
        // one currently rendered (e.g. a patch emitted by the bridge before a
        // session switch that arrives after the switch).
        if (patch.session.id !== prev.session.id || patch.revision < prev.revision) {
          scheduleFullResync();
          return prev;
        }
        // A revision gap means one or more patches were dropped between the
        // last accepted state and this one. Delta-only patches are only
        // meaningful when applied in-order with no gaps — after a gap the
        // stream store appends a suffix computed against an older backend
        // body, permanently misaligning the displayed text (the classic
        // "final part of the reply renders truncated"). Re-sync from a full
        // snapshot instead of merging a corrupted delta.
        if (patch.revision > prev.revision + 1) {
          scheduleFullResync();
          return prev;
        }

        prevSnapshotSessionId.current = patch.session.id;
        prevSnapshotRevision.current = Math.max(prev.revision, patch.revision);

        // Delta-only patches intentionally omit `messages`. Fold the live
        // stream store back into snapshot bodies so Idle/final renders keep
        // the full assistant text instead of a truncated prefix.
        if (isStreamingDeltaOnlyPatch(patch) || (hasDeltas && patch.messages.length === 0)) {
          return materializeStreamingMessageBodies({
            ...prev,
            revision: Math.max(prev.revision, patch.revision),
            session: patch.session,
            session_config: patch.session_config ?? prev.session_config,
            thinking_status: patch.thinking_status ?? prev.thinking_status,
            thinking_text: patch.thinking_text ?? prev.thinking_text,
            pending_steers: patch.pending_steers ?? prev.pending_steers,
          });
        }

        const next = applySnapshotPatch(prev, patch);
        return materializeStreamingMessageBodies(next);
      });
    })
      .then((cleanup) => {
        if (disposed) {
          cleanup();
          return;
        }
        unlistenPatch = cleanup;
      })
      .catch(() => {});

    return () => {
      disposed = true;
      if (reconcileTimer !== 0) {
        window.clearTimeout(reconcileTimer);
        reconcileTimer = 0;
      }
      unlisten?.();
      unlistenPatch?.();
    };
  }, [pollState]);

  // Periodic full-snapshot reconciliation. Catches patch loss that never
  // surfaces as a revision gap (e.g. the last deltas of a turn are dropped and
  // no further patch arrives to trigger the gap check): the backend revision
  // stays ahead of ours, so the next poll replaces the truncated local state
  // with the complete reply.
  useEffect(() => {
    if (!workspaceReady) return;
    let cancelled = false;
    const interval = window.setInterval(() => {
      // Probe the cheap revision endpoint first; only pay for a full snapshot
      // clone + serialization when the backend actually advanced. Long sessions
      // make `session_get_state` expensive, so this keeps the 3s poll light.
      sessionGetRevision()
        .then(([sessionId, revision]) => {
          if (cancelled) return;
          const changed =
            sessionId !== prevSnapshotSessionId.current ||
            revision !== prevSnapshotRevision.current;
          if (changed) void pollState();
        })
        .catch(() => {});
    }, SNAPSHOT_SELF_HEAL_POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(interval);
    };
  }, [workspaceReady, pollState]);

  useEffect(() => {
    if (!workspaceReady || snapshot) return;
    pollState();
  }, [pollState, snapshot, workspaceReady]);

  // Page older history (before the loaded window's earliest seq) and prepend
  // it into the local snapshot. Returns false when there's nothing older.
  const loadOlderHistory = useCallback(async (limit = 200): Promise<boolean> => {
    const current = snapshotRef.current;
    const earliest = current?.history_earliest_seq;
    if (!current || earliest == null) return false;
    try {
      const page = await sessionLoadHistoryBefore(earliest, limit);
      if (page.timeline.length === 0) return false;
      setSnapshot((prev) => {
        if (!prev) return prev;
        // Dedupe by id in case of overlap with the current window.
        const knownMessageIds = new Set(prev.messages.map((m) => m.id));
        const knownToolIds = new Set(prev.tools.map((t) => t.id));
        const knownTimeline = new Set(
          prev.timeline.map((item) =>
            typeof item === "object" && "Message" in item
              ? `m:${item.Message}`
              : typeof item === "object" && "Tool" in item
              ? `t:${item.Tool}`
              : String(item),
          ),
        );
        const newMessages = page.messages.filter((m) => !knownMessageIds.has(m.id));
        const newTools = page.tools.filter((t) => !knownToolIds.has(t.id));
        const newTimeline = page.timeline.filter((item) => {
          const key =
            typeof item === "object" && "Message" in item
              ? `m:${item.Message}`
              : typeof item === "object" && "Tool" in item
              ? `t:${item.Tool}`
              : String(item);
          return !knownTimeline.has(key);
        });
        return {
          ...prev,
          messages: [...newMessages, ...prev.messages],
          tools: [...newTools, ...prev.tools],
          timeline: [...newTimeline, ...prev.timeline],
          history_earliest_seq: page.has_more ? page.earliest_seq : null,
        };
      });
      return true;
    } catch {
      return false;
    }
  }, []);

  return {
    snapshot,
    setSnapshot,
    snapshotRef,
    workspaceReady,
    pollState,
    acceptSnapshot,
    clearSnapshot,
    clearWorkspace,
    loadOlderHistory,
  };
}
