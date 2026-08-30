// Line-level unified diff for the mobile file-diff sheet. The desktop review
// panel renders diffs with Monaco; the phone needs a dependency-free renderer
// over the same inputs (old_text/new_text). Strategy: trim the common
// prefix/suffix first (O(n)), then run an LCS DP over the remaining middle
// only — with a cell cap so a pathological huge middle degrades to a
// block-level replace (all dels then all adds) instead of exhausting memory.

export interface DiffRow {
  kind: "same" | "add" | "del";
  text: string;
}

/** A diff row with 1-based old/new line numbers (null on the absent side),
 * mirroring the desktop review diff's two number gutters. */
export interface NumberedDiffRow extends DiffRow {
  oldNo: number | null;
  newNo: number | null;
}

/** Attach old/new line numbers to raw diff rows. */
export function numberRows(rows: DiffRow[]): NumberedDiffRow[] {
  let oldNo = 0;
  let newNo = 0;
  return rows.map((row) => {
    if (row.kind === "same") {
      oldNo += 1;
      newNo += 1;
      return { ...row, oldNo, newNo };
    }
    if (row.kind === "del") {
      oldNo += 1;
      return { ...row, oldNo, newNo: null };
    }
    newNo += 1;
    return { ...row, oldNo: null, newNo };
  });
}

/** One rendered segment: a changed hunk (with its surrounding context) or a
 * collapsed run of unchanged lines. `gap` rows carry the hidden rows so the
 * gray "N 行未更改" block can expand in place — the desktop review behavior. */
export type DiffSegment =
  | { kind: "hunk"; heading: string; rows: NumberedDiffRow[] }
  | { kind: "gap"; count: number; rows: NumberedDiffRow[] };

/** Group numbered rows into hunks (every changed line ± `contextLines`,
 * overlapping windows merged — same algorithm as the desktop compaction) with
 * unchanged runs collapsed into expandable gaps. */
export function segmentHunks(rows: NumberedDiffRow[], contextLines: number = 3): DiffSegment[] {
  const changed = rows
    .map((row, index) => (row.kind === "same" ? -1 : index))
    .filter((index) => index >= 0);

  const ranges: { start: number; end: number }[] = [];
  for (const index of changed) {
    const start = Math.max(0, index - contextLines);
    const end = Math.min(rows.length, index + contextLines + 1);
    const last = ranges[ranges.length - 1];
    if (last && start <= last.end) {
      last.end = Math.max(last.end, end);
    } else {
      ranges.push({ start, end });
    }
  }

  const segments: DiffSegment[] = [];
  let cursor = 0;
  for (const range of ranges) {
    if (range.start > cursor) {
      const hidden = rows.slice(cursor, range.start);
      segments.push({ kind: "gap", count: hidden.length, rows: hidden });
    }
    const hunkRows = rows.slice(range.start, range.end);
    segments.push({ kind: "hunk", heading: hunkHeading(hunkRows), rows: hunkRows });
    cursor = range.end;
  }
  if (cursor < rows.length) {
    const hidden = rows.slice(cursor);
    segments.push({ kind: "gap", count: hidden.length, rows: hidden });
  }
  return segments;
}

/** `@@ -a,b +c,d @@` from the hunk's rows, matching the unified-diff header. */
function hunkHeading(rows: NumberedDiffRow[]): string {
  const first = rows[0];
  const last = rows[rows.length - 1];
  const oldStart = first.oldNo ?? Math.max(0, (last.oldNo ?? 0) - countKind(rows, "del"));
  const newStart = first.newNo ?? Math.max(0, (last.newNo ?? 0) - countKind(rows, "add"));
  const oldCount = rows.filter((row) => row.oldNo != null).length;
  const newCount = rows.filter((row) => row.newNo != null).length;
  return `@@ -${oldStart},${oldCount} +${newStart},${newCount} @@`;
}

function countKind(rows: NumberedDiffRow[], kind: DiffRow["kind"]): number {
  return rows.filter((row) => row.kind === kind).length;
}

/** Cells the DP matrix may use (old_mid * new_mid). ~1.6M cells ≈ 6.4MB. */
const MAX_DP_CELLS = 1_600_000;

function splitLines(text: string | null | undefined): string[] {
  if (!text) return [];
  const lines = text.split("\n");
  // A trailing newline produces a final empty element — it is not a line.
  if (lines.length > 0 && lines[lines.length - 1] === "") lines.pop();
  return lines;
}

export function computeLineDiff(oldText: string | null | undefined, newText: string | null | undefined): DiffRow[] {
  const oldLines = splitLines(oldText);
  const newLines = splitLines(newText);

  // Common prefix.
  let prefix = 0;
  while (
    prefix < oldLines.length &&
    prefix < newLines.length &&
    oldLines[prefix] === newLines[prefix]
  ) {
    prefix++;
  }
  // Common suffix (never overlapping the prefix).
  let suffix = 0;
  while (
    suffix < oldLines.length - prefix &&
    suffix < newLines.length - prefix &&
    oldLines[oldLines.length - 1 - suffix] === newLines[newLines.length - 1 - suffix]
  ) {
    suffix++;
  }

  const oldMid = oldLines.slice(prefix, oldLines.length - suffix);
  const newMid = newLines.slice(prefix, newLines.length - suffix);

  const rows: DiffRow[] = [];
  for (let i = 0; i < prefix; i++) rows.push({ kind: "same", text: oldLines[i] });

  if (oldMid.length === 0 && newMid.length === 0) {
    // Identical texts.
  } else if (oldMid.length === 0) {
    for (const text of newMid) rows.push({ kind: "add", text });
  } else if (newMid.length === 0) {
    for (const text of oldMid) rows.push({ kind: "del", text });
  } else if (oldMid.length * newMid.length > MAX_DP_CELLS) {
    // Too large for LCS — degrade to a block replace so the content is
    // never lost, just less granular.
    for (const text of oldMid) rows.push({ kind: "del", text });
    for (const text of newMid) rows.push({ kind: "add", text });
  } else {
    appendLcsRows(oldMid, newMid, rows);
  }

  for (let i = 0; i < suffix; i++) rows.push({ kind: "same", text: oldLines[oldLines.length - suffix + i] });
  return rows;
}

/** Classic LCS backtrack over Int32Array rows; pushes add/del/same rows in order. */
function appendLcsRows(a: string[], b: string[], rows: DiffRow[]): void {
  const n = a.length;
  const m = b.length;
  // dp[i][j] = LCS length of a[i..], b[j..] — stored row-major with (m+1) stride.
  const stride = m + 1;
  const dp = new Int32Array((n + 1) * stride);
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      dp[i * stride + j] =
        a[i] === b[j]
          ? dp[(i + 1) * stride + (j + 1)] + 1
          : Math.max(dp[(i + 1) * stride + j], dp[i * stride + (j + 1)]);
    }
  }
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (a[i] === b[j]) {
      rows.push({ kind: "same", text: a[i] });
      i++;
      j++;
    } else if (dp[(i + 1) * stride + j] >= dp[i * stride + (j + 1)]) {
      rows.push({ kind: "del", text: a[i] });
      i++;
    } else {
      rows.push({ kind: "add", text: b[j] });
      j++;
    }
  }
  while (i < n) rows.push({ kind: "del", text: a[i++] });
  while (j < m) rows.push({ kind: "add", text: b[j++] });
}
// end of file
