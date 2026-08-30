// Port of the desktop diff compaction (apps/desktop/ui
// `features/tooling/compact-patch.ts`): tool diff previews may carry whole
// files when the agent emits a full-file hunk, and rendering every line makes
// a large edit card take screens of scrolling. Like the desktop, keep each
// changed line plus DIFF_CONTEXT_LINES of context, merge the overlapping
// windows, and surface everything else as an explicit gap marker.

import type { DiffHunk } from "../../types";

export const DIFF_CONTEXT_LINES = 3;

export type CompactDiffRow =
  | { kind: "line"; lineKind: "Context" | "Added" | "Removed"; content: string }
  | { kind: "gap"; count: number };

export interface CompactHunk {
  /** Original `@@ -a,b +c,d @@` heading; only carried on the first segment. */
  heading: string | null;
  rows: CompactDiffRow[];
}

export function compactPreviewHunks(hunks: DiffHunk[]): CompactHunk[] {
  return hunks.map((hunk, hunkIndex) => {
    const ranges = compactRanges(hunk.lines);

    if (ranges.length === 0) {
      // No changed lines (defensive: the backend only emits hunks with
      // changes, but a pure-context hunk must not disappear entirely).
      // Mirror the desktop fallback: keep the first 12 lines of the hunk.
      const kept = hunk.lines.slice(0, 12);
      const rows: CompactDiffRow[] = kept.map(toRow);
      if (hunk.lines.length > kept.length) {
        rows.push({ kind: "gap", count: hunk.lines.length - kept.length });
      }
      return { heading: hunkIndex === 0 ? hunk.heading : null, rows };
    }

    const rows: CompactDiffRow[] = [];
    let cursor = 0;
    for (const range of ranges) {
      if (range.start > cursor) {
        rows.push({ kind: "gap", count: range.start - cursor });
      }
      for (const line of hunk.lines.slice(range.start, range.end)) {
        rows.push(toRow(line));
      }
      cursor = range.end;
    }
    if (cursor < hunk.lines.length) {
      rows.push({ kind: "gap", count: hunk.lines.length - cursor });
    }
    return { heading: hunkIndex === 0 ? hunk.heading : null, rows };
  });
}

function toRow(line: DiffHunk["lines"][number]): CompactDiffRow {
  return { kind: "line", lineKind: line.kind, content: line.content };
}

/** Keep windows of ±DIFF_CONTEXT_LINES around every changed line, clamped to
 * the hunk bounds and merged when they overlap — the desktop behavior. */
function compactRanges(lines: DiffHunk["lines"]): { start: number; end: number }[] {
  const changedIndexes = lines
    .map((line, index) => (line.kind === "Context" ? -1 : index))
    .filter((index) => index >= 0);

  const ranges: { start: number; end: number }[] = [];
  for (const index of changedIndexes) {
    const start = Math.max(0, index - DIFF_CONTEXT_LINES);
    const end = Math.min(lines.length, index + DIFF_CONTEXT_LINES + 1);
    const last = ranges[ranges.length - 1];
    if (last && start <= last.end) {
      last.end = Math.max(last.end, end);
    } else {
      ranges.push({ start, end });
    }
  }
  return ranges;
}
