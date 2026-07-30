import { useRef } from "react";
import { useFrame, useThree } from "@react-three/fiber";
import * as THREE from "three";
import type { CompanionMood } from "../state/types";
import type { GazeTarget } from "./CompanionAvatar";

interface PlaceholderAvatarProps {
  mood: CompanionMood;
  gaze: GazeTarget;
  lowPower: boolean;
}

/** mood → 主色调（与设计 brief 的深紫黑/品红/红缎带一致） */
const MOOD_COLORS: Record<CompanionMood, string> = {
  idle: "#8a5fa8",
  curious: "#9a6fc0",
  thinking: "#6a5a9a",
  working: "#5a5a8a",
  awaiting_permission: "#d0487a",
  happy: "#e0609a",
  frustrated: "#7a6a8a",
  pouty: "#b05070",
  sleepy: "#4a4a5e",
};

/**
 * MVP 程序化占位头像：VRM 资产未落地前（见 public/companion/LICENSE.md）
 * 以抽象球体 + 表情色驱动角色表现，渲染/交互管线与正式 VRM 完全一致。
 */
export function PlaceholderAvatar({ mood, gaze, lowPower }: PlaceholderAvatarProps) {
  const groupRef = useRef<THREE.Group>(null);
  const materialRef = useRef<THREE.MeshStandardMaterial>(null);
  const clockRef = useRef({ time: 0, breathPhase: 0 });
  const { invalidate } = useThree();

  useFrame((_, rawDt) => {
    if (!groupRef.current || !materialRef.current) return;
    const dt = Math.min(rawDt, 0.1);
    const clock = clockRef.current;
    clock.time += dt;

    const targetColor = new THREE.Color(MOOD_COLORS[mood]);
    materialRef.current.color.lerp(targetColor, Math.min(1, dt * 4));
    materialRef.current.emissive.lerp(targetColor.clone().multiplyScalar(0.25), Math.min(1, dt * 4));

    // 注视跟随（整体轻微转向）
    const targetYaw = THREE.MathUtils.clamp(gaze.x * 0.4, -0.4, 0.4);
    const targetPitch = THREE.MathUtils.clamp(-gaze.y * 0.2, -0.2, 0.2);
    groupRef.current.rotation.y = THREE.MathUtils.damp(groupRef.current.rotation.y, targetYaw, 6, dt);
    groupRef.current.rotation.x = THREE.MathUtils.damp(groupRef.current.rotation.x, targetPitch, 6, dt);

    if (!lowPower) {
      clock.breathPhase += dt * 1.6;
      groupRef.current.position.y = Math.sin(clock.breathPhase) * 0.03;
      if (mood === "happy") {
        groupRef.current.position.y = Math.abs(Math.sin(clock.time * 6)) * 0.12;
      } else if (mood === "sleepy") {
        groupRef.current.rotation.x = THREE.MathUtils.damp(groupRef.current.rotation.x, 0.3, 1.5, dt);
      }
      invalidate();
    }
  });

  return (
    <group ref={groupRef}>
      <mesh>
        <sphereGeometry args={[0.5, 32, 32]} />
        <meshStandardMaterial ref={materialRef} color={MOOD_COLORS.idle} roughness={0.35} metalness={0.1} />
      </mesh>
      {/* 心形高光暗示（占位：小球代替） */}
      <mesh position={[0.32, 0.42, 0.28]}>
        <sphereGeometry args={[0.07, 16, 16]} />
        <meshStandardMaterial color="#e0609a" emissive="#e0609a" emissiveIntensity={0.6} />
      </mesh>
    </group>
  );
}
