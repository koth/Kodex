import { useCallback, useEffect, useRef, useState } from "react";
import type { PointerEvent as ReactPointerEvent, MouseEvent as ReactMouseEvent } from "react";
import { useCompanion } from "./state/useCompanion";
import { CompanionCanvas } from "./scene/CompanionCanvas";
import { SpeechBubble } from "./ui/SpeechBubble";
import "./companion.css";

/** 点击互动节流（spec: 5s 内不重复触发） */
const CLICK_THROTTLE_MS = 5_000;
const COMPANION_WIDTH = 180;
const COMPANION_HEIGHT = 240;

/**
 * 陪伴角色悬浮层。
 * - 指针事件隔离：根容器 pointer-events:none，仅角色/气泡/控制钮命中区域响应
 * - 默认右下角停靠（settings.position 归一化坐标）
 * - 拖拽定位 + 最小化，均持久化
 */
export function CompanionLayer() {
  const {
    settings,
    state,
    bubble,
    lowPower,
    setSettings,
    notifyUserInteraction,
    dismissBubble,
  } = useCompanion();

  const [gaze, setGaze] = useState({ x: 0, y: 0 });
  const [hovered, setHovered] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const dragRef = useRef<{ startX: number; startY: number; baseX: number; baseY: number } | null>(null);
  const lastClickRef = useRef(0);
  const leaveTimerRef = useRef<number | null>(null);

  // 注视跟随：全局鼠标移动 → 归一化坐标
  useEffect(() => {
    if (!settings.enabled || settings.minimized) return;
    const handleMove = (event: MouseEvent) => {
      const x = (event.clientX / window.innerWidth) * 2 - 1;
      const y = (event.clientY / window.innerHeight) * 2 - 1;
      setGaze({ x, y });
      if (leaveTimerRef.current !== null) {
        window.clearTimeout(leaveTimerRef.current);
        leaveTimerRef.current = null;
      }
    };
    const handleLeave = () => {
      // 鼠标离开窗口 2s 后视线回正（spec: 注视跟随）
      leaveTimerRef.current = window.setTimeout(() => setGaze({ x: 0, y: 0 }), 2_000);
    };
    window.addEventListener("mousemove", handleMove);
    document.documentElement.addEventListener("mouseleave", handleLeave);
    return () => {
      window.removeEventListener("mousemove", handleMove);
      document.documentElement.removeEventListener("mouseleave", handleLeave);
      if (leaveTimerRef.current !== null) window.clearTimeout(leaveTimerRef.current);
    };
  }, [settings.enabled, settings.minimized]);

  const handleDragStart = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => {
      if (event.button !== 0) return;
      dragRef.current = {
        startX: event.clientX,
        startY: event.clientY,
        baseX: settings.position.x,
        baseY: settings.position.y,
      };
      event.currentTarget.setPointerCapture(event.pointerId);
    },
    [settings.position],
  );

  const handleDragMove = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => {
      const drag = dragRef.current;
      if (!drag) return;
      const dx = (event.clientX - drag.startX) / window.innerWidth;
      const dy = (event.clientY - drag.startY) / window.innerHeight;
      setSettings({
        ...settings,
        position: {
          x: Math.min(0.98, Math.max(0.02, drag.baseX + dx)),
          y: Math.min(0.98, Math.max(0.02, drag.baseY + dy)),
        },
      });
    },
    [settings, setSettings],
  );

  const handleDragEnd = useCallback(() => {
    dragRef.current = null;
  }, []);

  const handleClick = useCallback(
    (event: ReactMouseEvent<HTMLDivElement>) => {
      const now = Date.now();
      if (now - lastClickRef.current < CLICK_THROTTLE_MS) return;
      lastClickRef.current = now;
      event.stopPropagation();
      notifyUserInteraction();
    },
    [notifyUserInteraction],
  );

  const toggleMinimized = useCallback(() => {
    setSettings({ ...settings, minimized: !settings.minimized });
  }, [settings, setSettings]);

  if (!settings.enabled) return null;

  if (settings.minimized) {
    return (
      <button
        type="button"
        className="companion-minimized"
        style={{
          left: `${settings.position.x * 100}%`,
          top: `${settings.position.y * 100}%`,
        }}
        onClick={toggleMinimized}
        title="召唤陪伴角色"
      >
        ♡
      </button>
    );
  }

  return (
    <div
      className="companion-layer"
      style={{
        left: `${settings.position.x * 100}%`,
        top: `${settings.position.y * 100}%`,
        // Resize the layout box itself (not a visual transform scale) so an
        // enlarged avatar never overflows its container and gets clipped by
        // neighbouring panels. translate(-50%,-50%) from the stylesheet keeps
        // the box centered on the normalized dock position.
        width: COMPANION_WIDTH * settings.scale,
        height: COMPANION_HEIGHT * settings.scale,
      }}
    >
      <SpeechBubble text={bubble} mood={state.mood} onDismiss={dismissBubble} />
      <div className={`companion-body ${hovered ? "is-hovered" : ""} ${lowPower ? "is-low-power" : ""}`}>
        <div
          className="companion-hitarea"
          onPointerDown={handleDragStart}
          onPointerMove={handleDragMove}
          onPointerUp={handleDragEnd}
          onPointerCancel={handleDragEnd}
          onClick={handleClick}
          onPointerEnter={() => setHovered(true)}
          onPointerLeave={() => setHovered(false)}
          role="img"
          aria-label={`陪伴角色，当前状态：${state.mood}`}
        />
        <CompanionCanvas
          mood={state.mood}
          gaze={gaze}
          lowPower={lowPower}
          modelUrl={settings.modelUrl}
          onLoadError={setLoadError}
        />
        {loadError && (
          <div className="companion-load-error" role="alert">
            模型加载失败：{loadError}
          </div>
        )}
        <button
          type="button"
          className="companion-minimize-btn"
          onClick={(event) => {
            event.stopPropagation();
            toggleMinimized();
          }}
          title="最小化"
          aria-label="最小化陪伴角色"
        >
          −
        </button>
      </div>
    </div>
  );
}
