# 陪伴角色设定档（Character Bible）

锁定 Kodex 内置陪伴角色的视觉身份，作为所有资产生成 / 修改 / 3D 复刻的一致性锚点。
**改任何资产前先读本档；所有衍生图都必须与"身份锚点"保持一致。**

## 人设（Persona）

- 定位：轻病娇陪伴角色 —— 温柔乖巧、安静黏人，略带占有欲，绝不打扰工作。
- 语气三档：`gentle / standard / intense`（见 `src/features/companion/persona/lines.ts`）。
- 情绪状态机 9 种 mood：`idle / curious / thinking / working / awaiting_permission / happy / frustrated / pouty / sleepy`（见 `state/types.ts`）。

## 身份锚点（Identity Anchors）—— 不可漂移

| 部位 | 锁定描述 |
|---|---|
| 脸型 | 圆润鹅蛋脸，柔和下颌线 |
| 眼 | 深棕近黑杏眼，眼神温润、安静直视 |
| 眉 | 淡平眉 |
| 唇 | 自然粉唇，嘴角微上扬的浅笑 |
| 发 | 黑色长直发及腰；空气感齐刘海（轻薄略过眉）；两侧长须发垂落 |
| 气质 | 温柔乖巧、内敛，带一点"只看着你"的专注 |
| 穿搭 | 浅灰蓝针织开衫 + 天蓝色高领内搭 + 白色大圆扣；低饱和柔和配色 |
| 风格 | 偏真人感的半写实细腻插画风（非强二次元 / 非 Q 版） |

## 资产清单（Asset Inventory）

| 文件 | 用途 | 状态 |
|---|---|---|
| `ref/hero-v1.png` | 定妆照（**身份锚点**，一切衍生图的参考源） | ✅ 已锁定 |
| `ref/expressions-v1.png` | 9 格表情参考表（对应 9 种 mood） | ✅ 已生成 |
| `ref/turnaround-v1.png` | 三视图（正/侧/背），供 3D 素体建模参考 | ✅ 已生成 |
| `ref/portrait-green-src-v1.png` | 立绘 chroma-key 源图（绿底） | 中间产物 |
| `portrait/portrait-v1.png` | **透明背景全身立绘**（接 `CompanionCanvas` 无 WebGL 降级 portrait） | ✅ 可用 |
| `avatar/`（待做） | 聊天气泡 / 应用内小头像（方形） | ⬜ 待生成 |
| `model/`（待做） | 可驱动 .vrm 本体（humanoid + 标准表情预设） | ⬜ 3D 阶段 |

## 生成配方（Generation Recipes）

生图走 OpenAI images `edits` 端点（`gpt-image-2`），以 `ref/hero-v1.png` 为身份参考图（image→image）。
**核心原则：永远以 hero-v1 为参考源，不要在衍生图之间链式参考，避免漂移。**

通用约束段（每个 prompt 都带）：
```
Preserve her identity: round oval face, soft jawline, warm dark-brown almond eyes,
thin soft straight brows, natural pink lips, long straight jet-black hair with a
light wispy see-through fringe and long side strands. Style: polished semi-realistic
anime illustration, soft clean shading. Avoid: heavy stylization, chibi, exaggerated
expression, glasses, jewelry, text, watermark.
```

- **表情表** `size=1536x1024 quality=high`：9 格网格，情绪克制，参考 `MOOD_EXPRESSIONS`。
- **三视图** `size=1536x1024 quality=high`：正/侧/背并立，同姿势同身高对齐。
- **透明立绘** `size=1024x1536 quality=high`：纯色 `#00ff00` chroma-key 背景，再用
  `remove_chroma_key.py --auto-key border --soft-matte --transparent-threshold 12 --opaque-threshold 220 --despill` 转透明。
- **头像**（待做）：以 hero-v1 头部裁切 / 重新出方形 `1024x1024`，正面浅笑。

## 性能与许可约束

- 位图资产单张建议 ≤ 5MB（当前 hero 2.5MB / 立绘 0.7MB / 三视图 1.9MB）。
- 正式 `.vrm` 模型 ≤ 30MB、贴图 ≤ 2K（见 `LICENSE.md`）。
- 所有资产为 AI 生成 + 本人照片参考，无第三方版权素材；引入外部素材须登记进 `LICENSE.md`。

## 后续里程碑

1. ⬜ 头像（avatar）
2. ⬜ 把 `portrait-v1.png` 接进 `CompanionCanvas` 的无 WebGL 降级分支
3. ⬜ 3D 素体（VRoid / 可商用素体），按 `turnaround-v1.png` + hero 复刻
4. ⬜ 用本设定图重绘素体脸部 / 发色贴图，做出"专属感"
