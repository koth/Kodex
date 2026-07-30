import { Suspense, useEffect, useState } from "react";
import { Canvas } from "@react-three/fiber";
import type { CompanionMood } from "../state/types";
import { CompanionAvatar, type GazeTarget } from "./CompanionAvatar";
import { PlaceholderAvatar } from "./PlaceholderAvatar";
import { useVrmModel } from "./useVrmModel";

export type RenderTier = "vrm" | "placeholder" | "portrait" | "bubble-only";

interface CompanionCanvasProps {
  mood: CompanionMood;
  gaze: GazeTarget;
  lowPower: boolean;
  /** 默认模型 URL；null 或加载失败 → 程序化占位头像 */
  modelUrl: string | null;
  onTierChange?: (tier: RenderTier) => void;
  onLoadError?: (error: string | null) => void;
}

function detectWebGL(): boolean {
  if (typeof document === "undefined") return false;
  try {
    const canvas = document.createElement("canvas");
    return !!(
      canvas.getContext("webgl2") ??
      canvas.getContext("webgl") ??
      canvas.getContext("experimental-webgl")
    );
  } catch {
    return false;
  }
}

function CompanionScene({ mood, gaze, lowPower, modelUrl, onLoadError }: Omit<CompanionCanvasProps, "onTierChange">) {
  const { vrm, status, error } = useVrmModel(modelUrl);
  useEffect(() => {
    onLoadError?.(status === "error" ? error ?? "未知错误" : null);
  }, [status, error, onLoadError]);
  if (status === "ready" && vrm) {
    return <CompanionAvatar vrm={vrm} mood={mood} gaze={gaze} lowPower={lowPower} />;
  }
  return <PlaceholderAvatar mood={mood} gaze={gaze} lowPower={lowPower} />;
}

/**
 * R3F Canvas 封装：demand frameloop 按需渲染；
 * 降级链：WebGL 3D（VRM → 占位头像）→ 静态立绘 → 仅气泡。
 */
export function CompanionCanvas({ mood, gaze, lowPower, modelUrl, onTierChange, onLoadError }: CompanionCanvasProps) {
  const [webglAvailable, setWebglAvailable] = useState<boolean | null>(null);

  useEffect(() => {
    const available = detectWebGL();
    setWebglAvailable(available);
    onTierChange?.(available ? (modelUrl ? "vrm" : "placeholder") : "portrait");
  }, [modelUrl, onTierChange]);

  if (webglAvailable === null) return null;
  if (!webglAvailable) {
    return (
      <div className="companion-portrait-fallback" data-testid="companion-portrait-fallback">
        <span className={`companion-portrait-face companion-portrait-${mood}`} aria-hidden>
          {mood === "happy" ? "＾▽＾" : mood === "sleepy" ? "－ｗ－" : mood === "pouty" ? "＞＜" : "・ω・"}
        </span>
      </div>
    );
  }

  return (
    <Canvas
      frameloop="demand"
      dpr={[1, 1.5]}
      camera={{ position: [0, 0.9, 2.2], fov: 40 }}
      gl={{ antialias: true, alpha: true, powerPreference: "low-power" }}
      style={{ background: "transparent", pointerEvents: "none" }}
    >
      <ambientLight intensity={0.9} />
      <directionalLight position={[2, 3, 2]} intensity={1.1} />
      <Suspense fallback={null}>
        <CompanionScene mood={mood} gaze={gaze} lowPower={lowPower} modelUrl={modelUrl} onLoadError={onLoadError} />
      </Suspense>
    </Canvas>
  );
}
