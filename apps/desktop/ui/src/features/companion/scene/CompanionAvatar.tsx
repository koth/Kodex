import { useEffect, useMemo, useRef, useState } from "react";
import { useFrame, useThree } from "@react-three/fiber";
import * as THREE from "three";
import type { VRM } from "@pixiv/three-vrm";
import type { CompanionMood } from "../state/types";

export interface GazeTarget {
  /** 归一化鼠标位置（-1..1，相对画布） */
  x: number;
  y: number;
}

interface CompanionAvatarProps {
  vrm: VRM | null;
  mood: CompanionMood;
  gaze: GazeTarget;
  lowPower: boolean;
}

/** mood → VRM 表情预设权重 */
const MOOD_EXPRESSIONS: Record<CompanionMood, Record<string, number>> = {
  idle: { happy: 0.15 },
  curious: { surprised: 0.35 },
  thinking: { neutral: 0.6, sad: 0.1 },
  working: { neutral: 0.8, angry: 0.12 },
  awaiting_permission: { surprised: 0.55, happy: 0.25 },
  happy: { happy: 1.0 },
  frustrated: { sad: 0.9 },
  pouty: { angry: 0.55, sad: 0.25 },
  sleepy: { relaxed: 0.85 },
};

const EXPRESSION_LERP = 4; // 每秒插值速率
const GAZE_MAX_YAW = 0.5; // 颈部最大偏航（弧度）
const GAZE_MAX_PITCH = 0.3;
const BLINK_INTERVAL_MIN = 2.2;
const BLINK_INTERVAL_MAX = 5.5;
const BLINK_DURATION = 0.16;

/** 手臂自然下垂（绕 Z 轴内收，T-pose → 自然站姿）。VRM 骨骼归零即 T-pose，需向身体中线方向旋转。 */
const ARM_RELAX_Z = 1.15;
const IDLE_SWAY_AMOUNT = 0.03;

type HumanoidBoneName = "leftUpperArm" | "rightUpperArm" | "leftLowerArm" | "rightLowerArm" | "spine" | "chest";

function relaxArmPose(vrm: VRM, dt: number, sway: number) {
  const bones: Array<{ name: HumanoidBoneName; zSign: 1 | -1 }> = [
    { name: "leftUpperArm", zSign: 1 },
    { name: "rightUpperArm", zSign: -1 },
  ];
  for (const { name, zSign } of bones) {
    const bone = vrm.humanoid?.getNormalizedBoneNode(name);
    if (!bone) continue;
    const targetZ = zSign * (ARM_RELAX_Z + sway);
    bone.rotation.z = THREE.MathUtils.damp(bone.rotation.z, targetZ, 5, dt);
    // 轻微前收，避免僵直
    bone.rotation.x = THREE.MathUtils.damp(bone.rotation.x, 0.12, 5, dt);
  }
  // 前臂自然微曲
  for (const name of ["leftLowerArm", "rightLowerArm"] as const) {
    const bone = vrm.humanoid?.getNormalizedBoneNode(name);
    if (!bone) continue;
    bone.rotation.z = THREE.MathUtils.damp(bone.rotation.z, name.startsWith("left") ? 0.25 : -0.25, 5, dt);
  }
}

function damp(current: number, target: number, lambda: number, dt: number): number {
  return THREE.MathUtils.damp(current, target, lambda, dt);
}

export function CompanionAvatar({ vrm, mood, gaze, lowPower }: CompanionAvatarProps) {
  const groupRef = useRef<THREE.Group>(null);
  const { invalidate } = useThree();
  const [frame, setFrame] = useState<{ center: THREE.Vector3; height: number } | null>(null);
  const clockRef = useRef({
    time: 0,
    blinkTimer: BLINK_INTERVAL_MIN,
    blinking: 0,
    breathPhase: Math.random() * Math.PI * 2,
    moodBlend: new Map<string, number>(),
  });

  const expressions = useMemo(() => MOOD_EXPRESSIONS[mood] ?? {}, [mood]);

  // mood 变化时请求重绘（demand 模式）
  useEffect(() => {
    invalidate();
  }, [mood, gaze.x, gaze.y, invalidate]);

  useEffect(() => {
    if (!vrm) return;
    vrm.scene.rotation.y = Math.PI; // VRM 默认面向 +Z，转向相机
    // 自动取景：按模型包围盒把相机对准胸口、拉远到全身入镜
    const box = new THREE.Box3().setFromObject(vrm.scene);
    const size = box.getSize(new THREE.Vector3());
    const center = box.getCenter(new THREE.Vector3());
    setFrame({ center, height: Math.max(size.y, 0.5) });
  }, [vrm]);

  useFrame((_, rawDt) => {
    if (!vrm || !groupRef.current) return;
    const dt = Math.min(rawDt, 0.1);
    const clock = clockRef.current;
    clock.time += dt;

    // --- 表情混合 ---
    const manager = vrm.expressionManager;
    if (manager) {
      const names = new Set<string>([
        "happy",
        "sad",
        "angry",
        "surprised",
        "relaxed",
        "neutral",
      ]);
      for (const name of names) {
        const target = expressions[name] ?? 0;
        const current = clock.moodBlend.get(name) ?? 0;
        const next = damp(current, target, EXPRESSION_LERP, dt);
        clock.moodBlend.set(name, next);
        try {
          manager.setValue(name, next);
        } catch {
          // 模型缺少该表情预设时静默跳过
        }
      }

      // --- 眨眼（低功耗下仍保留，成本极低） ---
      clock.blinkTimer -= dt;
      if (clock.blinkTimer <= 0 && clock.blinking <= 0) {
        clock.blinking = BLINK_DURATION;
        clock.blinkTimer =
          BLINK_INTERVAL_MIN + Math.random() * (BLINK_INTERVAL_MAX - BLINK_INTERVAL_MIN);
      }
      if (clock.blinking > 0) {
        clock.blinking -= dt;
        const phase = 1 - Math.abs(clock.blinking / BLINK_DURATION - 0.5) * 2;
        try {
          manager.setValue("blink", mood === "sleepy" ? Math.max(phase, 0.55) : phase);
        } catch {
          // ignore
        }
      }
    }

    // --- 注视跟随（头部骨骼平滑插值 + 角度限制） ---
    const head = vrm.humanoid?.getNormalizedBoneNode("head");
    if (head && !lowPower) {
      const targetYaw = THREE.MathUtils.clamp(gaze.x * GAZE_MAX_YAW, -GAZE_MAX_YAW, GAZE_MAX_YAW);
      const targetPitch = THREE.MathUtils.clamp(-gaze.y * GAZE_MAX_PITCH, -GAZE_MAX_PITCH, GAZE_MAX_PITCH);
      // 模型已 rotation.y=π 面向相机，鼠标右移（gaze.x>0）应向右转头（正 yaw）
      head.rotation.y = damp(head.rotation.y, targetYaw, 6, dt);
      head.rotation.x = damp(head.rotation.x, targetPitch, 6, dt);
    }

    // --- 程序化动画 ---
    if (!lowPower) {
      clock.breathPhase += dt * 1.6;
      const breath = Math.sin(clock.breathPhase) * 0.012;
      groupRef.current.position.y = breath;

      // 手臂姿态：自然下垂 + idle 微动（mood 相关的上半身姿态）
      const sway = Math.sin(clock.time * 0.8) * IDLE_SWAY_AMOUNT;
      if (mood === "awaiting_permission") {
        // 双手合十前倾：上臂前抬
        for (const side of ["leftUpperArm", "rightUpperArm"] as const) {
          const bone = vrm.humanoid?.getNormalizedBoneNode(side);
          if (bone) {
            bone.rotation.x = THREE.MathUtils.damp(bone.rotation.x, -0.9, 5, dt);
            bone.rotation.z = THREE.MathUtils.damp(bone.rotation.z, side.startsWith("left") ? 0.5 : -0.5, 5, dt);
          }
        }
      } else if (mood === "happy") {
        // 开心：小臂轻快上抬
        for (const side of ["leftUpperArm", "rightUpperArm"] as const) {
          const bone = vrm.humanoid?.getNormalizedBoneNode(side);
          if (bone) {
            bone.rotation.x = THREE.MathUtils.damp(bone.rotation.x, -0.4, 5, dt);
            bone.rotation.z = THREE.MathUtils.damp(bone.rotation.z, side.startsWith("left") ? 0.7 : -0.7, 5, dt);
          }
        }
      } else if (mood === "pouty" || mood === "frustrated") {
        // 嘟嘴/沮丧：双臂环抱收紧
        for (const side of ["leftUpperArm", "rightUpperArm"] as const) {
          const bone = vrm.humanoid?.getNormalizedBoneNode(side);
          if (bone) {
            bone.rotation.x = THREE.MathUtils.damp(bone.rotation.x, 0.35, 5, dt);
            bone.rotation.z = THREE.MathUtils.damp(bone.rotation.z, side.startsWith("left") ? 0.6 : -0.6, 5, dt);
          }
        }
      } else {
        relaxArmPose(vrm, dt, mood === "idle" ? sway : 0);
      }

      // happy 时轻微起伏（小跳跃感）
      if (mood === "happy") {
        groupRef.current.position.y = Math.abs(Math.sin(clock.time * 6)) * 0.05;
      } else if (mood === "sleepy") {
        // 打瞌睡：头部缓慢下沉
        if (head) head.rotation.x = damp(head.rotation.x, 0.35, 1.5, dt);
      } else if (mood === "pouty") {
        // 撇开视线
        if (head) head.rotation.y = damp(head.rotation.y, 0.4, 3, dt);
      } else if (mood === "thinking") {
        if (head) head.rotation.z = damp(head.rotation.z, 0.12, 3, dt);
      } else if (head) {
        head.rotation.z = damp(head.rotation.z, 0, 3, dt);
      }
      invalidate();
    }

    vrm.update(dt);
  });

  if (!vrm) return null;
  return (
    <group ref={groupRef}>
      <primitive object={vrm.scene} />
      {frame && <AutoFramingCamera target={frame.center} height={frame.height} />}
    </group>
  );
}

/** 相机自动取景：全身 + 少量头部留白 */
function AutoFramingCamera({ target, height }: { target: THREE.Vector3; height: number }) {
  const { camera, invalidate } = useThree();
  useEffect(() => {
    const distance = height * 1.35;
    camera.position.set(target.x, target.y + height * 0.05, target.z + distance);
    camera.lookAt(target.x, target.y + height * 0.05, target.z);
    if (camera instanceof THREE.PerspectiveCamera) {
      camera.near = distance / 100;
      camera.far = distance * 20;
      camera.updateProjectionMatrix();
    }
    invalidate();
  }, [camera, target, height, invalidate]);
  return null;
}
