import { describe, expect, it } from "vitest";
import { repairCompactMarkdown } from "../features/conversation/repair-compact-markdown";

// Ports the key cases of the desktop repairCompactMarkdown behavior so the
// phone repairs assistant output exactly like the PC before markdown parsing:
// compact tables, escaped headings, run-on numbered lists, literal \n breaks,
// and compact (no-space) code fences.

describe("repairCompactMarkdown", () => {
  it("expands a compact single-line table into real GFM rows", () => {
    const out = repairCompactMarkdown("指标|A|B||---|---|---||速度|快|慢||稳定性|低|高");
    expect(out.split("\n")).toEqual(["|指标|A|B|", "|---|---|---|", "|速度|快|慢|", "|稳定性|低|高|"]);
  });

  it("repairs a two-line compact table (header line + separator-only body line)", () => {
    // Mirrors the desktop split-table repair: the leading fragment becomes a
    // paragraph prefix, the table starts at the first pipe column.
    const out = repairCompactMarkdown("对比|A|B\n|---|---|---||速度|快|慢");
    expect(out.split("\n")).toEqual(["对比", "", "|A|B|", "|---|---|---|", "|速度|快|慢|"]);
  });

  it("repairs an escaped heading `\\#\\# 标题` into `## 标题`", () => {
    expect(repairCompactMarkdown("\\#\\# 标题")).toBe("## 标题");
  });

  it("splits run-on numbered lists after prose (non-space before the counter)", () => {
    expect(repairCompactMarkdown("结论1. 第一条 2. 第二条")).toBe("结论\n1. 第一条 2. 第二条");
    expect(repairCompactMarkdown("第一条2. 第二条")).toBe("第一条\n2. 第二条");
  });

  it("turns literal \\n escapes into real newlines for markdown-ish bodies", () => {
    const out = repairCompactMarkdown("第一行\\n第二行\\n- 列表项");
    expect(out).toBe("第一行\n第二行\n- 列表项");
  });

  it("repairs compact fences with a language and no space", () => {
    const out = repairCompactMarkdown("```pythonprint('hi')```");
    expect(out).toBe("```python\nprint('hi')\n```");
  });

  it("keeps fence content untouched (no list/heading repair inside fences)", () => {
    const input = "```\n# not a heading\n1. not a list\n```";
    expect(repairCompactMarkdown(input)).toBe(input);
  });

  it("unwraps a stringified markdown payload wrapped in quotes", () => {
    const inner = "## 标题\n- 列表项";
    expect(repairCompactMarkdown(JSON.stringify(inner))).toBe(inner);
  });

  it("leaves plain prose untouched", () => {
    const prose = "两个根因都找到了: 1. adv_t.std() 2. logits range";
    expect(repairCompactMarkdown(prose)).toBe(prose);
  });
});
// end of file