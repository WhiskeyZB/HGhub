import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const host = process.env.TAURI_DEV_HOST;
const { version } = JSON.parse(
  readFileSync(new URL("./package.json", import.meta.url), "utf8"),
) as { version: string };

export default defineConfig(({ mode }) => {
  // Web 服务版（--mode web）：把 @tauri-apps/api 的两个模块换成本地垫片
  // （invoke → HTTP、window → 空实现），其余构建与客户端版完全一致。
  const web = mode === "web";
  return {
    plugins: [react(), tailwindcss()],
    clearScreen: false,
    // 版本号只维护 package.json（CI 会按 tag 改写它），界面里的版本由这里注入，避免写死后漂移
    define: {
      __APP_VERSION__: JSON.stringify(version),
    },
    resolve: web
      ? {
          alias: [
            {
              find: "@tauri-apps/api/core",
              replacement: fileURLToPath(
                new URL("./src/web/tauriCore.ts", import.meta.url),
              ),
            },
            {
              find: "@tauri-apps/api/window",
              replacement: fileURLToPath(
                new URL("./src/web/tauriWindow.ts", import.meta.url),
              ),
            },
          ],
        }
      : undefined,
    server: {
      port: 1420,
      strictPort: true,
      host: host || false,
      hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
      watch: { ignored: ["**/src-tauri/**"] },
    },
    envPrefix: ["VITE_", "TAURI_ENV_*"],
    build: {
      target: "es2021",
      minify: "esbuild",
      sourcemap: false,
    },
  };
});
