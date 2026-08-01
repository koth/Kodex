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
  const glowRef = useRef<THREE.MeshStandardMaterial>(null);
  const clockRef = useRef({
    time: 0,
    breathPhase: Math.random() * Math.PI * 2,
    swayPhase: Math.random() * Math.PI * 2,
    pulsePhase: Math.random() * Math.PI * 2,
    // 待机小动作：绕自身中垂轴轻微摆动着随机选一个幅度
    fidgetPhase: Math.random() * Math.PI * 2,
  });
  const { invalidate } = useThree();

  useFrame((_, rawDt) => {
    if (!groupRef.current || !materialRef.current) return;
    const dt = Math.min(rawDt, 0.1);
    const clock = clockRef.current;
    clock.time += dt;
    clock.breathPhase += dt * 1.6;
    clock.swayPhase += dt * 0.8;
    clock.pulsePhase += dt * (mood === "happy" ? 8 : 3);
    clock.fidgetPhase += dt * 1.1;

    const targetColor = new THREE.Color(MOOD_COLORS[mood]);
    materialRef.current.color.lerp(targetColor, Math.min(1, dt * 4));
    materialRef.current.emissive.lerp(targetColor.clone().multiplyScalar(0.25), Math.min(1, dt * 4));
    if (glowRef.current) {
      // 心跳式光晕脉冲：情绪越强脉冲越快/越亮
      const pulse = (Math.sin(clock.pulsePhase) + 1) / 2;
      const intensity = 0.35 + pulse * (mood === "happy" || mood === "awaiting_permission" ? 0.8 : 0.45);
      glowRef.current.emissiveIntensity = THREE.MathUtils.damp(
        glowRef.current.emissiveIntensity,
        intensity,
        4,
        dt,
      );
    }

    // 注视跟随（整体轻微转向），叠加自然的待机摆动
    const targetYaw = THREE.MathUtils.clamp(gaze.x * 0.4, -0.4, 0.4);
    const targetPitch = THREE.MathUtils.clamp(-gaze.y * 0.2, -0.2, 0.2);
    const idleYaw = Math.sin(clock.fidgetPhase) * 0.18;
    const idlePitch = Math.sin(clock.swayPhase) * 0.08;
    groupRef.current.rotation.y = THREE.MathUtils.damp(
      groupRef.current.rotation.y,
      targetYaw + (mood === "idle" ? idleYaw : 0),
      6,
      dt,
    );
    const pitchTarget =
      mood === "sleepy"
        ? 0.3
        : mood === "thinking"
          ? targetPitch + 0.12
          : targetPitch + (mood === "idle" ? idlePitch : 0);
    groupRef.current.rotation.x = THREE.MathUtils.damp(groupRef.current.rotation.x, pitchTarget, 4, dt);

    if (!lowPower) {
      // 呼吸浮沉 + 待机的上下轻盈浮动
      const floatY = Math.sin(clock.fidgetPhase) * 0.04;
      groupRef.current.position.y = Math.sin(clock.breathPhase) * 0.03;
      if (mood === "happy") {
        groupRef.current.position.y = Math.abs(Math.sin(clock.time * 6)) * 0.12;
      } else if (mood === "pouty" || mood === "frustrated") {
        // 泄气：整体沉下去一点
        groupRef.current.position.y = Math.sin(clock.breathPhase) * 0.015 - 0.05;
      } else if (mood === "curious" || mood === "awaiting_permission") {
        groupRef.current.position.y = Math.sin(clock.breathPhase) * 0.04 + floatY;
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
        <meshStandardMaterial ref={glowRef} color="#e0609a" emissive="#e0609a" emissiveIntensity={0.6} />
      </mesh>
    </group>
  );
}
