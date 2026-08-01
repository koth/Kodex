// 一次性探查脚本：解析 VRM (GLB) 提取 humanoid 骨骼的局部变换，
// 用来为 CCD 交叠姿态设定合理的肘部极向量/手部目标，避免穿模。
import fs from "node:fs";

const filePath = process.argv[2];
if (!filePath) {
  console.error("usage: node inspect-vrm.mjs <path-to-vrm>");
  process.exit(1);
}
const buf = fs.readFileSync(filePath);

// GLB header: magic(4) version(4) length(4)
const magic = buf.readUInt32LE(0);
if (magic !== 0x46546c67) {
  console.error("not a GLB (magic mismatch)");
  process.exit(1);
}
// chunk 0 header: chunkLength(4) chunkType(4)
const chunkLength = buf.readUInt32LE(12);
const chunkType = buf.readUInt32LE(16);
if (chunkType !== 0x4e4f534a) {
  console.error("first chunk is not JSON");
  process.exit(1);
}
const json = JSON.parse(buf.subarray(20, 20 + chunkLength).toString("utf8"));

const nodes = json.nodes || [];
const vrm0 = json.extensions && json.extensions.VRM;
const humanBones = vrm0 && vrm0.humanoid && vrm0.humanoid.humanBones;

console.log("=== VRM meta ===");
console.log("nodes:", nodes.length);
if (json.asset) console.log("asset:", JSON.stringify(json.asset));

// helper: read a node's transform
function nodeInfo(idx) {
  const n = nodes[idx];
  return {
    idx,
    name: n.name,
    translation: n.translation || [0, 0, 0],
    rotation: n.rotation || [0, 0, 0, 1],
    scale: n.scale || [1, 1, 1],
    children: n.children || [],
  };
}

if (humanBones) {
  console.log("\n=== humanBones (VRM 0.x) ===");
  for (const hb of humanBones) {
    const info = nodeInfo(hb.node);
    console.log(
      `${hb.bone.padEnd(16)} node=${info.idx} name=${info.name} t=[${info.translation.map((v) => v.toFixed(3)).join(", ")}]`
    );
  }
} else {
  console.log("\nno VRM0 humanoid; printing nodes that look like arm/leg bones:");
  const armRe = /(upper_?arm|lower_?arm|shoulder|hand|arm|elbow|wrist|hips|spine|chest|neck|head|thigh|leg)/i;
  nodes.forEach((n, idx) => {
    if (n.name && armRe.test(n.name)) {
      const info = nodeInfo(idx);
      console.log(`${String(idx).padStart(4)} ${info.name} t=[${info.translation.map((v) => v.toFixed(3)).join(", ")}]`);
    }
  });
}

// also print full node tree roots for orientation context
const childSet = new Set();
nodes.forEach((n) => (n.children || []).forEach((c) => childSet.add(c)));
const roots = nodes.map((_, i) => i).filter((i) => !childSet.has(i));
console.log("\n=== scene roots ===");
console.log("root node indices:", roots.join(", "));

// print hierarchy for arm chain to understand parent frames
function findByName(regex) {
  return nodes.map((n, i) => ({ n, i })).filter(({ n }) => n.name && regex.test(n.name));
}
const hips = findByName(/hips|pelvis|root/i);
console.log("\n=== candidate hips/root ===");
hips.slice(0, 6).forEach(({ n, i }) => console.log(i, n.name));
