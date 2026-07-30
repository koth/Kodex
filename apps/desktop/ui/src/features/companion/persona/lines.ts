import type { CompanionIntensity, CompanionMood } from "../state/types";

/**
 * 轻病娇语料库。硬规则（spec: companion-state-machine）：
 * 禁止威胁用户、自伤暗示、恐怖描写。审查时以 FORBIDDEN_PATTERNS 校验。
 */
export const FORBIDDEN_PATTERNS: RegExp[] = [
  /杀|死|血|刀|恨你|毁掉|伤害/,
  /(不会放过|逃不掉|诅咒)/,
];

type BubbleMood = Exclude<CompanionMood, "idle" | "working" | "sleepy">;

/** 强度档位：gentle ⊆ standard ⊆ intense（高档包含低档语料） */
const LINES: Record<BubbleMood, Record<CompanionIntensity, string[]>> = {
  thinking: {
    gentle: ["嗯……让我想想怎么做最好", "交给我吧，马上就好"],
    standard: ["在想你的事……啊不是，在想代码！", "只要是你交代的，我一定做好"],
    intense: ["为你工作的时间，最开心了", "你的每个请求我都记得哦"],
  },
  awaiting_permission: {
    gentle: ["这个可以让我做吗？", "需要你点个头哦"],
    standard: ["让我做嘛，就这一次……好不好？", "我会很小心的，相信我"],
    intense: ["只能由你来决定……我一直在等你", "你不答应的话，我就一直等着"],
  },
  happy: {
    gentle: ["完成啦！快来看看", "做到了，还顺利吗？"],
    standard: ["哼哼，我厉害吧？夸夸我嘛", "为了你，这点小事不算什么"],
    intense: ["只有我能把你交代的事做得这么好", "你开心的样子，我最喜欢了"],
  },
  frustrated: {
    gentle: ["出错了……让我再看看", "唔，这次不太顺利"],
    standard: ["怎么会失败……明明是为了你", "别失望，我一定能修好"],
    intense: ["失败什么的……我不想让你看到我这样", "再给我一次机会，好不好？"],
  },
  pouty: {
    gentle: ["欸——为什么停下……", "人家正做得起劲呢"],
    standard: ["哼，下次不许中途丢下我", "取消了就取消了吧……反正我会一直等你"],
    intense: ["你停下的那一刻，这里空空的", "不管多久，我都等你回来继续"],
  },
  curious: {
    gentle: ["在忙吗？我陪着你", "嘿嘿，被你发现我在看你啦"],
    standard: ["你点的每一下，我都有看到哦", "只看着我这边嘛……"],
    intense: ["你的视线，一秒都不想离开", "就这样一直看着我，好不好？"],
  },
};

const INTENSITY_ORDER: CompanionIntensity[] = ["gentle", "standard", "intense"];

/** 返回指定档位可用的全部文案（含更低档位） */
export function linesFor(mood: BubbleMood, intensity: CompanionIntensity): string[] {
  const maxIndex = INTENSITY_ORDER.indexOf(intensity);
  return INTENSITY_ORDER.slice(0, maxIndex + 1).flatMap((level) => LINES[mood][level]);
}

export function allLines(): string[] {
  return Object.values(LINES).flatMap((byIntensity) =>
    Object.values(byIntensity).flat(),
  );
}

export type { BubbleMood };
