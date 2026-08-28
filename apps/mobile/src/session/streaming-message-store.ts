// In-memory store of streaming message bodies keyed by message id, mirroring
// the desktop `conversation/streaming-message-store.ts`. `message_deltas` are
// appended here for smooth streaming until the full body lands in a patch.
// The first delta for a message is seeded with the snapshot's current body
// (the desktop equivalent is `ensureStreamingMessageBody` at render time), so
// an append is always computed against the text the user can already see.

const bodies = new Map<string, string>();

export function appendStreamingMessageDelta(
  id: string,
  append: string,
  seedBody?: string,
): void {
  if (!append) return;
  const current = bodies.get(id);
  if (current === undefined) {
    // First delta for this message: the chain starts from the snapshot body.
    bodies.set(id, (seedBody ?? "") + append);
    return;
  }
  bodies.set(id, current + append);
}

export function getStreamingMessageBody(id: string): string | null {
  return bodies.get(id) ?? null;
}

/** Reconcile an EXISTING streaming entry with an authoritative snapshot body:
 * a body that diverged (message replaced/rewritten) resets to the snapshot so
 * later deltas append to the right base. Never seeds empty entries — only
 * messages that already receive deltas are tracked. */
export function reconcileStreamingMessageBody(id: string, body: string): void {
  const current = bodies.get(id);
  if (current === undefined || current === body) return;
  if (current.startsWith(body) || body.startsWith(current)) return; // same lineage
  bodies.set(id, body);
}

export function clearStreamingMessage(id: string): void {
  bodies.delete(id);
}

export function clearAllStreamingMessages(): void {
  bodies.clear();
}
// end of file