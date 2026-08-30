import { describe, expect, it } from "vitest";
import { computeLineDiff } from "../features/tooling/line-diff";

describe("computeLineDiff", () => {
  it("returns no rows for identical text", () => {
    expect(computeLineDiff("a\nb\nc", "a\nb\nc")).toEqual([
      { kind: "same", text: "a" },
      { kind: "same", text: "b" },
      { kind: "same", text: "c" },
    ]);
  });

  it("marks pure additions", () => {
    const rows = computeLineDiff("a\nc", "a\nb\nc");
    expect(rows).toEqual([
      { kind: "same", text: "a" },
      { kind: "add", text: "b" },
      { kind: "same", text: "c" },
    ]);
  });

  it("marks pure deletions", () => {
    const rows = computeLineDiff("a\nb\nc", "a\nc");
    expect(rows).toEqual([
      { kind: "same", text: "a" },
      { kind: "del", text: "b" },
      { kind: "same", text: "c" },
    ]);
  });

  it("handles a replaced line as del followed by add", () => {
    const rows = computeLineDiff("old", "new");
    expect(rows).toEqual([
      { kind: "del", text: "old" },
      { kind: "add", text: "new" },
    ]);
  });

  it("treats empty old text as an all-add (new file)", () => {
    const rows = computeLineDiff(null, "x\ny");
    expect(rows).toEqual([
      { kind: "add", text: "x" },
      { kind: "add", text: "y" },
    ]);
  });

  it("treats empty new text as an all-delete (removed file)", () => {
    const rows = computeLineDiff("x\ny", "");
    expect(rows).toEqual([
      { kind: "del", text: "x" },
      { kind: "del", text: "y" },
    ]);
  });

  it("keeps a trailing newline from creating a phantom line", () => {
    expect(computeLineDiff("a\n", "a\n")).toEqual([{ kind: "same", text: "a" }]);
  });

  it("degrades huge middles to block replace without throwing", () => {
    const bigOld = Array.from({ length: 2000 }, (_, i) => `old ${i}`).join("\n");
    const bigNew = Array.from({ length: 2000 }, (_, i) => `new ${i}`).join("\n");
    const rows = computeLineDiff(bigOld, bigNew);
    const dels = rows.filter((r) => r.kind === "del").length;
    const adds = rows.filter((r) => r.kind === "add").length;
    expect(dels).toBe(2000);
    expect(adds).toBe(2000);
  });
});

import { numberRows, segmentHunks, type NumberedDiffRow } from "../features/tooling/line-diff";

describe("numberRows + segmentHunks (review-aligned rendering)", () => {
  const text = Array.from({ length: 20 }, (_, i) => `line ${i + 1}`).join("\n");
  const changed = text.replace("line 12", "line 12 changed");

  it("numbers old and new sides independently", () => {
    const rows = numberRows(computeLineDiff("a\nb\nc", "a\nX\nc"));
    expect(rows).toEqual([
      { kind: "same", text: "a", oldNo: 1, newNo: 1 },
      { kind: "del", text: "b", oldNo: 2, newNo: null },
      { kind: "add", text: "X", oldNo: null, newNo: 2 },
      { kind: "same", text: "c", oldNo: 3, newNo: 3 },
    ]);
  });

  it("collapses unchanged runs into gaps and keeps context around changes", () => {
    const rows = numberRows(computeLineDiff(text, changed));
    const segments = segmentHunks(rows, 3);
    // One changed line in the middle → exactly one hunk plus gaps on both sides.
    const hunks = segments.filter((s) => s.kind === "hunk");
    const gaps = segments.filter((s) => s.kind === "gap");
    expect(hunks).toHaveLength(1);
    expect(gaps).toHaveLength(2);
    const hunk = hunks[0];
    if (hunk.kind !== "hunk") throw new Error("unreachable");
    // 3 context lines before + del + add (one replaced line) + 3 after.
    expect(hunk.rows).toHaveLength(8);
    expect(hunk.rows.some((r) => r.kind === "del")).toBe(true);
    expect(hunk.rows.some((r) => r.kind === "add")).toBe(true);
    expect(hunk.heading).toMatch(/^@@ -\d+,\d+ \+\d+,\d+ @@$/);
    const gapCounts = gaps.map((g) => (g.kind === "gap" ? g.count : 0));
    // 21 rendered rows (20 old lines, one replaced line contributes del+add):
    // window [8,16) leaves 8 hidden before and 5 hidden after.
    expect(gapCounts).toEqual([8, 5]);
    // Hidden rows are preserved so the gap can expand in place.
    expect(gaps[0].kind === "gap" && gaps[0].rows).toHaveLength(8);
  });

  it("expanding all gaps restores the full file", () => {
    const rows = numberRows(computeLineDiff(text, changed));
    const segments = segmentHunks(rows, 3);
    const rendered: NumberedDiffRow[] = [];
    for (const segment of segments) {
      rendered.push(...segment.rows);
    }
    expect(rendered).toHaveLength(21); // 20 old lines + 1 extra (del+add)
  });
});
