import { describe, expect, it } from "vitest";
import { allLines, FORBIDDEN_PATTERNS, linesFor } from "./lines";
import { LinePicker, pickLine } from "./pickLine";

describe("persona lines 合规校验", () => {
  it("全部文案不包含威胁/自伤/恐怖关键词", () => {
    for (const line of allLines()) {
      for (const pattern of FORBIDDEN_PATTERNS) {
        expect(pattern.test(line), `违规文案: ${line}`).toBe(false);
      }
    }
  });

  it("强度档位为包含关系：gentle ⊆ standard ⊆ intense", () => {
    const gentle = linesFor("happy", "gentle");
    const standard = linesFor("happy", "standard");
    const intense = linesFor("happy", "intense");
    expect(gentle.length).toBeGreaterThan(0);
    expect(standard.length).toBeGreaterThan(gentle.length);
    expect(intense.length).toBeGreaterThan(standard.length);
    for (const line of gentle) expect(standard).toContain(line);
  });
});

describe("pickLine 去重", () => {
  it("最近 5 次内不重复", () => {
    const picker = new LinePicker();
    const seen: string[] = [];
    let seq = 0;
    const random = () => (seq++ % 7) / 7;
    for (let i = 0; i < 6; i++) {
      seen.push(picker.pick("happy", "intense", random));
    }
    const recent = seen.slice(-5);
    expect(new Set(recent).size).toBe(recent.length);
  });

  it("纯函数版本同样避免 recent 重复", () => {
    const random = () => 0;
    const first = pickLine("pouty", "standard", [], random);
    const second = pickLine("pouty", "standard", [first], random);
    expect(second).not.toBe(first);
  });
});
