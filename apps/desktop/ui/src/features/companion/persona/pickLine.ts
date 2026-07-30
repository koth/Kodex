import { linesFor, type BubbleMood } from "./lines";
import type { CompanionIntensity } from "../state/types";

const RECENT_WINDOW = 5;

/**
 * 从语料库选取一条气泡文案。
 * 规则：最近 RECENT_WINDOW 次内出现过的文案不再重复；全部用罄时重置窗口。
 */
export class LinePicker {
  private recent: string[] = [];

  pick(mood: BubbleMood, intensity: CompanionIntensity, random: () => number = Math.random): string {
    const candidates = linesFor(mood, intensity);
    const fresh = candidates.filter((line) => !this.recent.includes(line));
    const pool = fresh.length > 0 ? fresh : candidates;
    const line = pool[Math.floor(random() * pool.length)];
    this.recent.push(line);
    if (this.recent.length > RECENT_WINDOW) {
      this.recent.shift();
    }
    return line;
  }
}

export function pickLine(
  mood: BubbleMood,
  intensity: CompanionIntensity,
  recent: readonly string[],
  random: () => number = Math.random,
): string {
  const candidates = linesFor(mood, intensity);
  const fresh = candidates.filter((line) => !recent.includes(line));
  const pool = fresh.length > 0 ? fresh : candidates;
  return pool[Math.floor(random() * pool.length)];
}
