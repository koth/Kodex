import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

const host = process.env.TAURI_DEV_HOST;

export default defineConfig({
  // .vrm/.vrma 作为静态资源导入（companion 角色资产管线）
  // 注意：three / @react-three/fiber / @react-three/drei / @pixiv/three-vrm
  // 的版本组合与 three 的 peer 依赖耦合（fiber@8 对应 react@18），
  // 升级 three 时必须同步验证其余三者兼容性。
  assetsInclude: ["**/*.vrm", "**/*.vrma"],
  plugins: [
    react(),
    {
      name: "tauri-cors-fix",
      enforce: "post",
      transformIndexHtml(html) {
        return html.replace(/\s*crossorigin\s*/g, " ");
      },
    },
  ],
  clearScreen: false,
  base: "./",
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
});
