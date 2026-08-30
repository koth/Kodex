import { describe, expect, it } from "vitest";
import { compactPreviewHunks, DIFF_CONTEXT_LINES } from "../features/tooling/compact-diff";
import type { DiffHunk } from "../types";

// Mirrors the desktop compact-patch behavior: changed lines ± 3 context
// lines, overlapping windows merged, dropped runs surfaced as gap markers.

function hunk(lines: Array<[DiffHunk["lines"][number]["kind"], string]>, heading = "@@ -1,10 +1,10 @@"): DiffHunk {
  return { heading, lines: lines.map(([kind, content]) => ({ kind, content })) };
}

describe("compactPreviewHunks", () => {
  it("keeps ±3 context lines around a single change and collapses the rest", () => {
    const lines: Array<[DiffHunk["lines"][number]["kind"], string]> = [
      ["Context", `l${1}`],
      ["Context", "l2"],
      ["Context", "l3"],
      ["Context", "l4"],
      ["Context", "l5"],
      ["Context", "l6"],
      ["Context", "l7"],
      ["Context", "l8"],
      ["Context", "l9"],
      ["Context", "l10"],
      ["Added", "new line"],
    ];
    const [result] = compactPreviewHunks([hunk(lines)]);
    expect(result?.heading).toBe("@@ -1,10 +1,10 @@");
    // Window around the added line = l8..l10 + added; l1..l7 collapse.
    expect(result?.rows[0]).toEqual({ kind: "gap", count: 7 });
    expect(result?.rows[1]).toEqual({ kind: "line", lineKind: "Context", content: "l8" });
    expect(result?.rows[3]).toEqual({ kind: "line", lineKind: "Context", content: "l10" });
    expect(result?.rows.at(-1)).toEqual({ kind: "line", lineKind: "Added", content: "new line" });
  });

  it("merges overlapping windows into one range with no gap", () => {
    const lines: Array<[DiffHunk["lines"][number]["kind"], string]> = [
      ["Context", "c1"],
      ["Added", "a1"],
      // 4 context lines: the ±3 windows around a1 and r1 overlap → one range.
      ["Context", "m1"],
      ["Context", "m2"],
      ["Context", "m3"],
      ["Context", "m4"],
      ["Removed", "r1"],
      ["Context", "c2"],
    ];
    const [result] = compactPreviewHunks([hunk(lines)]);
    // Merged window covers every line: nothing is collapsed.
    expect(result?.rows.every((row) => row.kind === "line")).toBe(true);
    expect(result?.rows.filter((row) => row.kind === "line")).toHaveLength(8);
  });

  it("splits separate ranges with a gap marker between them", () => {
    const lines: Array<[DiffHunk["lines"][number]["kind"], string]> = [
      ["Added", "a1"],
      // 8 context lines: a1's window is [0,4), a2's is [6,10) → truly separate.
      ["Context", "x1"],
      ["Context", "x2"],
      ["Context", "x3"],
      ["Context", "x4"],
      ["Context", "x5"],
      ["Context", "x6"],
      ["Context", "x7"],
      ["Context", "x8"],
      ["Added", "a2"],
    ];
    const [result] = compactPreviewHunks([hunk(lines)]);
    expect(result?.heading).toBe("@@ -1,10 +1,10 @@");
    expect(result?.rows[0]).toEqual({ kind: "line", lineKind: "Added", content: "a1" });
    expect(result?.rows[3]).toEqual({ kind: "line", lineKind: "Context", content: "x3" });
    expect(result?.rows[4]).toEqual({ kind: "gap", count: 2 });
    expect(result?.rows[5]).toEqual({ kind: "line", lineKind: "Context", content: "x6" });
    expect(result?.rows.at(-1)).toEqual({ kind: "line", lineKind: "Added", content: "a2" });
    expect(result?.rows).toHaveLength(9); // 4 lines + 1 gap + 4 lines
  });

  it("keeps small hunks fully intact without gaps", () => {
    const [result] = compactPreviewHunks([
      hunk([
        ["Context", "ctx"],
        ["Added", "added"],
        ["Context", "ctx2"],
      ]),
    ]);
    expect(result?.rows).toEqual([
      { kind: "line", lineKind: "Context", content: "ctx" },
      { kind: "line", lineKind: "Added", content: "added" },
      { kind: "line", lineKind: "Context", content: "ctx2" },
    ]);
    expect(result?.heading).toBe("@@ -1,10 +1,10 @@");
  });

  it("falls back to the first 12 lines for a hunk without changes", () => {
    const lines: Array<[DiffHunk["lines"][number]["kind"], string]> = Array.from(
      { length: 30 },
      (_, i) => ["Context", `line${i}`] as [DiffHunk["lines"][number]["kind"], string],
    );
    const [result] = compactPreviewHunks([hunk(lines)]);
    expect(result?.rows.filter((row) => row.kind === "line")).toHaveLength(12);
    expect(result?.rows.at(-1)).toEqual({ kind: "gap", count: 18 });
  });

  it("only keeps the heading on the first hunk of a preview", () => {
    const [first, second] = compactPreviewHunks([
      hunk([["Added", "a"]], "@@ -1,2 +1,2 @@"),
      hunk([["Added", "b"]], "@@ -20,2 +20,2 @@"),
    ]);
    expect(first?.heading).toBe("@@ -1,2 +1,2 @@");
    expect(second?.heading).toBeNull();
  });
});
