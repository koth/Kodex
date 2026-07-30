import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { UiSnapshot } from "../../../types";
import { onUiSnapshot, onUiSnapshotPatch } from "../../../lib/events";
import type { CompanionEvent, CompanionSettings, CompanionState } from "./types";
import {
  initialCompanionState,
  moodAutoSettles,
  transition,
  MOOD_SETTLE_MS,
} from "./companionStateMachine";
import {
  createBridgeState,
  mapSnapshot,
  checkIdleTimeout,
  noteUserInteraction,
  IDLE_TIMEOUT_MS,
} from "./companionBridge";
import { LinePicker } from "../persona/pickLine";
import type { BubbleMood } from "../persona/lines";
import {
  loadCompanionSettings,
  saveCompanionSettings,
  onCompanionSettingsChanged,
} from "./companionSettingsStore";

const IDLE_CHECK_INTERVAL_MS = 15_000;
const BUBBLE_MOODS: ReadonlySet<string> = new Set([
  "thinking",
  "awaiting_permission",
  "happy",
  "frustrated",
  "pouty",
  "curious",
]);

export interface CompanionController {
  settings: CompanionSettings;
  state: CompanionState;
  bubble: string | null;
  lowPower: boolean;
  setSettings: (next: CompanionSettings) => void;
  notifyUserInteraction: () => void;
  dismissBubble: () => void;
}

/** 会话事件 → 状态机 → 气泡 的核心 hook（仅 CompanionLayer 使用） */
export function useCompanion(): CompanionController {
  const [settings, setSettingsState] = useState<CompanionSettings>(loadCompanionSettings);
  const [state, setState] = useState<CompanionState>(() => initialCompanionState(Date.now()));
  const [bubble, setBubble] = useState<string | null>(null);
  const [lowPower, setLowPower] = useState(false);

  const bridgeRef = useRef(createBridgeState(Date.now()));
  const pickerRef = useRef(new LinePicker());
  const intensityRef = useRef(settings.intensity);
  intensityRef.current = settings.intensity;
  const settleTimerRef = useRef<number | null>(null);

  const setSettings = useCallback((next: CompanionSettings) => {
    setSettingsState(next);
    saveCompanionSettings(next);
  }, []);

  // 设置页（另一个实例）变更后同步到本 hook
  useEffect(() => {
    return onCompanionSettingsChanged((next) => setSettingsState(next));
  }, []);

  const dispatch = useCallback((event: CompanionEvent) => {
    setState((current) => {
      const result = transition(current, event, Date.now());
      if (result.state === current) return current;
      if (result.showBubble && BUBBLE_MOODS.has(result.state.mood)) {
        const line = pickerRef.current.pick(
          result.state.mood as BubbleMood,
          intensityRef.current,
        );
        setBubble(line);
      }
      return result.state;
    });
  }, []);

  // 订阅会话快照（与 useWorkbenchSnapshot 相同的事件源）
  useEffect(() => {
    if (!settings.enabled) return;
    let cancelled = false;
    const unlisteners: Array<() => void> = [];

    const handleSnapshot = (snapshot: UiSnapshot) => {
      const { state: bridgeState, event } = mapSnapshot(bridgeRef.current, snapshot, Date.now());
      bridgeRef.current = bridgeState;
      if (event) {
        setLowPower(false);
        dispatch(event);
      }
    };

    onUiSnapshot(handleSnapshot).then((unlisten) => {
      if (cancelled) unlisten();
      else unlisteners.push(unlisten);
    });
    onUiSnapshotPatch((patch) => handleSnapshot(patch as unknown as UiSnapshot)).then(
      (unlisten) => {
        if (cancelled) unlisten();
        else unlisteners.push(unlisten);
      },
    );

    return () => {
      cancelled = true;
      for (const unlisten of unlisteners) unlisten();
    };
  }, [settings.enabled, dispatch]);

  // 空闲检测：5 分钟无活动 → sleepy 低功耗
  useEffect(() => {
    if (!settings.enabled) return;
    const timer = window.setInterval(() => {
      const event = checkIdleTimeout(bridgeRef.current, Date.now());
      if (event) {
        setLowPower(true);
        dispatch(event);
      }
    }, IDLE_CHECK_INTERVAL_MS);
    return () => window.clearInterval(timer);
  }, [settings.enabled, dispatch]);

  // 情绪自动回落：happy/frustrated/pouty/curious 停留 MOOD_SETTLE_MS 后回 idle
  useEffect(() => {
    if (settleTimerRef.current !== null) {
      window.clearTimeout(settleTimerRef.current);
      settleTimerRef.current = null;
    }
    if (!moodAutoSettles(state.mood)) return;
    settleTimerRef.current = window.setTimeout(() => {
      dispatch({ kind: "mood_settled" });
      setBubble(null);
    }, MOOD_SETTLE_MS);
    return () => {
      if (settleTimerRef.current !== null) {
        window.clearTimeout(settleTimerRef.current);
        settleTimerRef.current = null;
      }
    };
  }, [state.mood, state.enteredAt, dispatch]);

  const notifyUserInteraction = useCallback(() => {
    setLowPower(false);
    const event = noteUserInteraction(bridgeRef.current, Date.now());
    dispatch(event);
  }, [dispatch]);

  const dismissBubble = useCallback(() => setBubble(null), []);

  return useMemo(
    () => ({ settings, state, bubble, lowPower, setSettings, notifyUserInteraction, dismissBubble }),
    [settings, state, bubble, lowPower, setSettings, notifyUserInteraction, dismissBubble],
  );
}

export { IDLE_TIMEOUT_MS };
