import { useEffect, useMemo, useRef, useState } from "react";
import { useFrame, useThree } from "@react-three/fiber";
import * as THREE from "three";
import type { VRM, VRMHumanBoneName } from "@pixiv/three-vrm";
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

// 姿态/手势的平滑进入/退出速率（越小越柔）
const GESTURE_LERP = 4;

/** 目标姿态：给骨骼一个 3 分量旋转目标（弧度）。缺省字段原位不动。 */
interface BonePose {
  x?: number;
  y?: number;
  z?: number;
}
type PoseTarget = Partial<Record<VRMHumanBoneName, BonePose>>;

/**
 * mood 对应的上半身姿态刻画（数据驱动）。
 * z 符号约定（面向相机）：角色自身左侧骨骼用正 z，右侧用负 z。
 */
const MOOD_POSES: Record<CompanionMood, PoseTarget> = {
  // 女士待机的自然站姿：手臂自然下垂，双手在身前交叉合拢
  // （右手搭在左手背上），垂在腹部/膝上方附近，手掌放松内扣。
  idle: {
    chest: { x: 0.08 }, // 微含胸放松
  },
  curious: {
    head: { z: 0.12 },
    spine: { x: 0.06 },
  },
  thinking: {
    head: { z: 0.12, y: 0.1 },
    chest: { x: 0.12 },
    leftUpperArm: { x: -0.05, z: 0.75 },
    rightUpperArm: { x: -0.05, z: -0.75 },
    leftLowerArm: { x: 0.3, z: 0.35 },
    rightLowerArm: { x: 0.3, z: -0.35 },
  },
  working: {
    chest: { x: 0.1 },
    head: { x: -0.08 },
  },
  awaiting_permission: {
    // 双手合十前倾
    chest: { x: 0.2 },
    head: { x: -0.1 },
    leftUpperArm: { x: -0.9, z: 0.5 },
    rightUpperArm: { x: -0.9, z: -0.5 },
    leftLowerArm: { x: -0.15 },
    rightLowerArm: { x: -0.15 },
  },
  happy: {
    chest: { x: -0.12 },
    head: { x: -0.08 },
    leftUpperArm: { x: -0.45, z: 0.75 },
    rightUpperArm: { x: -0.45, z: -0.75 },
    leftLowerArm: { x: 0.2, z: 0.25 },
    rightLowerArm: { x: 0.2, z: -0.25 },
  },
  frustrated: {
    // 双臂环抱收紧、含胸
    chest: { x: 0.22 },
    head: { x: 0.05 },
    leftUpperArm: { x: 0.4, z: 0.6 },
    rightUpperArm: { x: 0.4, z: -0.6 },
    leftLowerArm: { x: -0.35, z: 0.3 },
    rightLowerArm: { x: -0.35, z: -0.3 },
  },
  pouty: {
    chest: { x: 0.18 },
    head: { y: 0.4, x: 0.06 },
    leftUpperArm: { x: 0.4, z: 0.6 },
    rightUpperArm: { x: 0.4, z: -0.6 },
    leftLowerArm: { x: -0.3, z: 0.3 },
    rightLowerArm: { x: -0.3, z: -0.3 },
  },
  sleepy: {
    // 打瞌睡：头沉、肩垮
    head: { x: 0.35 },
    chest: { x: 0.22 },
    spine: { x: 0.14 },
    leftLowerArm: { x: 0.4, z: 0.2 },
    rightLowerArm: { x: 0.4, z: -0.2 },
  },
};

/** 待机时的极轻微「调整姿态」小动作。刻意克制、低频、慢速进出，
 *  且只作用于近中立的小角度，避免任何表演性的突兀挥手。 */
interface IdleGesture {
  /** 总时长（s）：进/停/出三段平滑插值 */
  duration: number;
  /** 动作期间对相关骨骼施加的、相对于中立的小偏移（rad）。 */
  offset: PoseTarget;
}

const IDLE_GESTURES: IdleGesture[] = [
  // 头微微转向一侧，又转回来（扫视周围环境）
  { duration: 2.4, offset: { head: { y: 0.16 }, neck: { y: 0.08 } } },
  // 微微耸一下肩又放松（只在肩，不带动手臂/手腕开合）
  { duration: 2.6, offset: { leftShoulder: { x: -0.1 }, rightShoulder: { x: -0.1 } } },
  // 身子往左轻轻一倾（髋/脊柱/胸同向倾斜）
  {
    duration: 2.4,
    offset: { hips: { z: 0.07 }, spine: { z: 0.05 }, chest: { z: 0.05 }, head: { z: -0.03 } },
  },
  // 身子往右轻轻一倾（反向）
  {
    duration: 2.4,
    offset: { hips: { z: -0.07 }, spine: { z: -0.05 }, chest: { z: -0.05 }, head: { z: 0.03 } },
  },
  // 歪一点点头，像是回想什么
  { duration: 2.3, offset: { head: { z: 0.1 } } },
];

interface GestureRuntime {
  spec: IdleGesture;
  /** 剩余等待时间（s），到 0 进入动作 */
  wait: number;
  /** 0..1 归一化进度；0=进，1=出完毕 */
  t: number;
}

/** 所有 mood 共享的「自然下垂」基础姿态（T-pose 归零后会僵直，需先自然站姿四肢）。
 *  手腕/手掌给一个稳定的放松内收（固定值，不随呼吸张合，避免"一直张开合拢"）。 */
const RELAXED_BASE: PoseTarget = {
  leftUpperArm: { z: 1.15, x: 0.12 },
  rightUpperArm: { z: -1.15, x: 0.12 },
  leftLowerArm: { z: 0.22 },
  rightLowerArm: { z: -0.22 },
  leftHand: { x: 0.12, z: 0.05 },
  rightHand: { x: 0.12, z: -0.05 },
};

function damp(current: number, target: number, lambda: number, dt: number): number {
  return THREE.MathUtils.damp(current, target, lambda, dt);
}

// ---- 双手交叠（右手搭左手背）的解析两骨 IK ----
// 不依赖每个模型不同的骨骼轴向：直接在世界空间把两只手"引导"到身前
// 下腹部中线附近（右手在上、左手在下），并显式指定肘部极向量
// （肘部向下并略向外），避免肘部乱折、手臂穿过裙子/身体。

const _v5 = new THREE.Vector3();
const _v6 = new THREE.Vector3();
const _q1 = new THREE.Quaternion();

/** 把一个世界空间的旋转 delta 作用于骨（折算到父坐标系），并按 weight 融合。 */
function applyWorldRotation(joint: THREE.Object3D, worldDelta: THREE.Quaternion, weight: number) {
  const parent = joint.parent as THREE.Object3D;
  parent.updateWorldMatrix(true, false);
  const parentWorld = parent.getWorldQuaternion(new THREE.Quaternion());
  const local = parentWorld.clone().invert().multiply(worldDelta).multiply(parentWorld);
  const blended = new THREE.Quaternion().slerp(local, Math.min(1, weight));
  joint.quaternion.premultiply(blended);
  joint.updateWorldMatrix(true, false);
}

/**
 * 单骨"瞄准"：只旋转 forearm（肘→手），让 hand 指向 target。
 * 上臂不动（保持自然下垂贴在身体两侧），这样大臂不会被按进裙子/胸口，
 * 只有前臂向前弯、让双手在身前并拢——女士"手搭在前面"的自然站姿。
 */
function aimForearmAt(
  forearm: THREE.Object3D,
  hand: THREE.Object3D,
  target: THREE.Vector3,
  weight: number,
) {
  forearm.updateWorldMatrix(true, false);
  hand.updateWorldMatrix(false, false);
  const elbow = forearm.getWorldPosition(new THREE.Vector3());
  const wrist = hand.getWorldPosition(new THREE.Vector3());
  const toWrist = _v5.copy(wrist).sub(elbow);
  const toTarget = _v6.copy(target).sub(elbow);
  if (toWrist.lengthSq() < 1e-8 || toTarget.lengthSq() < 1e-8) return;
  toWrist.normalize();
  toTarget.normalize();
  _q1.setFromUnitVectors(toWrist, toTarget);
  applyWorldRotation(forearm, _q1, weight);
}

export function CompanionAvatar({ vrm, mood, gaze, lowPower }: CompanionAvatarProps) {
  const groupRef = useRef<THREE.Group>(null);
  const { invalidate, camera } = useThree();
  const [frame, setFrame] = useState<{ center: THREE.Vector3; height: number } | null>(null);
  const clockRef = useRef({
    time: 0,
    blinkTimer: BLINK_INTERVAL_MIN,
    blinking: 0,
    breathPhase: Math.random() * Math.PI * 2,
    swayPhase: Math.random() * Math.PI * 2,
    moodBlend: new Map<string, number>(),
    gesture: null as GestureRuntime | null,
  });

  const expressions = useMemo(() => MOOD_EXPRESSIONS[mood] ?? {}, [mood]);
  const moodPose = useMemo(() => MOOD_POSES[mood] ?? {}, [mood]);

  // mood 变化时：复位进行中的待机手势，避免情绪切换时残留动作；请求重绘
  useEffect(() => {
    if (clockRef.current.gesture) {
      clockRef.current.gesture = null;
    }
    invalidate();
  }, [mood, invalidate]);

  useEffect(() => {
    invalidate();
  }, [gaze.x, gaze.y, invalidate]);

  useEffect(() => {
    if (!vrm) return;
    vrm.scene.rotation.y = Math.PI; // VRM 默认面向 +Z，转向相机
    // 自动取景：按模型包围盒把相机对准胸口、拉远到全身入镜
    const box = new THREE.Box3().setFromObject(vrm.scene);
    const size = box.getSize(new THREE.Vector3());
    const center = box.getCenter(new THREE.Vector3());
    setFrame({ center, height: Math.max(size.y, 0.5) });
  }, [vrm]);

  /** 待机动作的进出相位包络（0..1）：进、停、出。 */
  const gestureEnvelope = (t: number): number => {
    const p = THREE.MathUtils.clamp(t, 0, 1);
    // 前 30% 淡入，后 30% 淡出，中间保持——都经 smoothstep 减轻突兀感
    if (p < 0.3) return smoothstep(p / 0.3);
    if (p > 0.7) return 1 - smoothstep((p - 0.7) / 0.3);
    return 1;
  };

  /**
   * 平滑驱动单根骨骼：
   *   自然放松基准 → mood 静态姿态 → 待机小动作的克制偏移 → 注视跟随 → 阻尼写入。
   * 手臂/手腕只做稳定的放松姿态，不做反复张合的摆动；
   * 身体的"活"由整体呼吸浮沉（group.position.y）承担，避免硬控手臂。
   */
  const driveBone = (vrm: VRM, name: VRMHumanBoneName, dt: number) => {
    const bone = vrm.humanoid?.getNormalizedBoneNode(name);
    if (!bone) return;
    const base = RELAXED_BASE[name] ?? {};
    const moodTarget = moodPose[name];
    const gesture = clockRef.current.gesture;
    const gestureOffset = gesture ? gesture.spec.offset[name] : undefined;

    // 1) 放松基准 + mood 静态姿态
    let tx = moodTarget?.x ?? base.x ?? 0;
    let ty = moodTarget?.y ?? base.y ?? 0;
    let tz = moodTarget?.z ?? base.z ?? 0;

    // 2) 待机小动作的克制偏移（近中立、低频）
    if (gesture && gestureOffset) {
      const w = gestureEnvelope(gesture.t) * (mood === "idle" ? 1 : 0);
      tx += (gestureOffset.x ?? 0) * w;
      ty += (gestureOffset.y ?? 0) * w;
      tz += (gestureOffset.z ?? 0) * w * (name.startsWith("right") ? -1 : 1);
    }

    // 3) 头部叠加注视跟随
    if (name === "head") {
      const targetYaw = THREE.MathUtils.clamp(gaze.x * GAZE_MAX_YAW, -GAZE_MAX_YAW, GAZE_MAX_YAW);
      const targetPitch = THREE.MathUtils.clamp(-gaze.y * GAZE_MAX_PITCH, -GAZE_MAX_PITCH, GAZE_MAX_PITCH);
      ty += targetYaw;
      tx += targetPitch;
    }

    bone.rotation.x = damp(bone.rotation.x, tx, GESTURE_LERP, dt);
    bone.rotation.y = damp(bone.rotation.y, ty, GESTURE_LERP, dt);
    bone.rotation.z = damp(bone.rotation.z, tz, GESTURE_LERP, dt);
  };

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

    if (lowPower) {
      // 低功耗：仅呼吸浮动，不驱动手臂/手势
      clock.breathPhase += dt * 1.6;
      groupRef.current.position.y = Math.sin(clock.breathPhase) * 0.012;
      vrm.update(dt);
      return;
    }

    // --- 待机手势调度（仅 idle 且非低功耗） ---
    if (mood === "idle") {
      if (!clock.gesture) {
        // 每次动作后歇 3~6s 再挑下一个，节奏更慢更自然
        const delay = 3 + Math.random() * 3;
        clock.gesture = {
          spec: IDLE_GESTURES[Math.floor(Math.random() * IDLE_GESTURES.length)],
          wait: delay,
          t: 1, // 尚未开始
        };
      }
    } else if (clock.gesture) {
      clock.gesture = null;
    }

    // --- 身体基础动作：呼吸起伏 + 重心微摆（比单个 sin 丰富） ---
    clock.breathPhase += dt * 1.6;
    clock.swayPhase += dt * 0.7;
    const gesture = clock.gesture;
    if (gesture) {
      if (gesture.wait > 0) {
        gesture.wait -= dt;
        if (gesture.wait <= 0) gesture.t = 0; // 开始动作
      } else {
        gesture.t = Math.min(1, gesture.t + dt / gesture.spec.duration);
        if (gesture.t >= 1) {
          clock.gesture = null; // 动作结束，等待重新调度
        }
      }
    }

    // 呼吸基础浮沉
    groupRef.current.position.y = Math.sin(clock.breathPhase) * 0.012;

    const group = groupRef.current;

    // 轻微自然重心摇晃（yaw 在 ±0.02 内），idle 与 curious/happy 更明显
    const swayAmount = mood === "idle" || mood === "curious" || mood === "happy" ? 0.025 : 0;
    const swayYaw = Math.sin(clock.swayPhase) * swayAmount;
    group.rotation.y = damp(group.rotation.y, swayYaw, 3, dt);

    // --- 躯干/手臂姿态：mood 姿态 + 待机手势叠加 ---
    const torsoBones: VRMHumanBoneName[] = [
      "hips",
      "spine",
      "chest",
      "upperChest",
      "neck",
      "head",
      "leftShoulder",
      "rightShoulder",
      "leftUpperArm",
      "rightUpperArm",
      "leftLowerArm",
      "rightLowerArm",
      "leftHand",
      "rightHand",
    ];
    for (const name of torsoBones) {
      // idle 时只跳过前臂：上臂由姿态表保持自然下垂贴身，手腕由姿态表
      // 给放松微弯；前臂交给下方的"瞄准"把双手在身前并拢，避免上臂被按进裙子。
      if (mood === "idle" && name.includes("LowerArm")) continue;
      driveBone(vrm, name, dt);
    }

    // idle：上臂保持自然下垂贴身，只弯前臂把双手在身前并拢（右手搭左手背）。
    // 只动前臂 + 世界坐标目标，不穿模、不依赖各模型轴向。
    if (mood === "idle") {
      const hips = vrm.humanoid?.getNormalizedBoneNode("hips");
      const lLower = vrm.humanoid?.getNormalizedBoneNode("leftLowerArm");
      const lHand = vrm.humanoid?.getNormalizedBoneNode("leftHand");
      const rLower = vrm.humanoid?.getNormalizedBoneNode("rightLowerArm");
      const rHand = vrm.humanoid?.getNormalizedBoneNode("rightHand");
      if (hips && lLower && lHand && rLower && rHand) {
        hips.updateWorldMatrix(true, false);
        const hipsPos = hips.getWorldPosition(new THREE.Vector3());
        // 模型面向相机 → 身前方向 = 从髋部水平指向相机
        const front = camera.position.clone().sub(hipsPos);
        front.y = 0;
        front.normalize();
        const rightAxis = new THREE.Vector3().crossVectors(front, new THREE.Vector3(0, 1, 0)).normalize();
        // 交叠点：髋部前方、低于髋部（下腹部/裙子上沿之外），两手在此并拢
        const center = hipsPos
          .clone()
          .add(front.clone().multiplyScalar(0.08))
          .add(new THREE.Vector3(0, -0.12, 0));
        const lTarget = center.clone().add(rightAxis.clone().multiplyScalar(0.03));
        const rTarget = center.clone().add(rightAxis.clone().multiplyScalar(-0.03)).add(new THREE.Vector3(0, 0.02, 0));
        const weight = Math.min(1, dt * 4); // 平滑靠拢
        aimForearmAt(lLower, lHand, lTarget, weight);
        aimForearmAt(rLower, rHand, rTarget, weight);
      }
    }

    // happy 时轻微起伏（小跳跃感）；sleepy/pouty/thinking 由 mood 姿态接管头/身
    if (mood === "happy") {
      group.position.y += Math.abs(Math.sin(clock.time * 6)) * 0.05;
    }

    vrm.update(dt);
    invalidate();
  });

  if (!vrm) return null;
  return (
    <group ref={groupRef}>
      <primitive object={vrm.scene} />
      {frame && <AutoFramingCamera target={frame.center} height={frame.height} />}
    </group>
  );
}

/** 平滑阶跃（smoothstep），用于动作进出，避免运动瞬间跳变。 */
function smoothstep(t: number): number {
  const p = THREE.MathUtils.clamp(t, 0, 1);
  return p * p * (3 - 2 * p);
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
