/** 角色情绪状态（对应 specs/companion-state-machine） */
export type CompanionMood =
  | "idle"
  | "curious"
  | "thinking"
  | "working"
  | "awaiting_permission"
  | "happy"
  | "frustrated"
  | "pouty"
  | "sleepy";

/** 会话侧输入事件（由 companionBridge 从 UiSnapshot/Tauri 事件映射而来） */
export type CompanionEvent =
  | { kind: "prompt_started" }
  | { kind: "tool_running"; toolName: string }
  | { kind: "permission_requested" }
  | { kind: "prompt_completed" }
  | { kind: "prompt_failed"; error?: string }
  | { kind: "prompt_cancelled" }
  | { kind: "idle_timeout" }
  | { kind: "user_interaction" }
  | { kind: "mood_settled" };

export type CompanionIntensity = "gentle" | "standard" | "intense";

export interface CompanionSettings {
  enabled: boolean;
  intensity: CompanionIntensity;
  /** 归一化停靠位置（0..1，相对工作台可视区） */
  position: { x: number; y: number };
  minimized: boolean;
  /** 用户自定义 VRM 模型 URL；null 使用内置默认 */
  modelUrl: string | null;
  /** 显示缩放比例（1 = 100%） */
  scale: number;
  /** 首次开启说明是否已确认 */
  introAcknowledged: boolean;
}

export const DEFAULT_COMPANION_SETTINGS: CompanionSettings = {
  enabled: false,
  intensity: "gentle",
  position: { x: 0.92, y: 0.85 },
  minimized: false,
  modelUrl: null,
  scale: 1,
  introAcknowledged: false,
};

/** 状态机快照（渲染层消费） */
export interface CompanionState {
  mood: CompanionMood;
  /** 触发气泡的文案 key；null 表示不弹气泡 */
  bubble: string | null;
  /** 进入当前 mood 的时间戳（ms），供渲染层做停留/衰减 */
  enteredAt: number;
}
