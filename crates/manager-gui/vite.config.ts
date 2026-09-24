import { defineConfig } from "vite";
import solid from "vite-plugin-solid";
// @ts-expect-error type error without @types/node package
import process from "node:process";
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(({ mode }) => ({
  // vite-plugin-solid enables its HMR transform whenever `command === "serve"`
  // and `mode !== "production"` — true under Vitest too, since Vitest runs
  // Vite with mode "test". That transform injects an import of the plugin's
  // "/@solid-refresh" virtual module, which only a real Vite dev server can
  // serve; under Vitest's Node-based runner it's an unparseable path on
  // Windows, crashing every component test before it runs (Linux/macOS
  // happened to tolerate the bogus path). `hot: false` in test mode turns
  // the transform off, the same as passing `--mode production` would.
  plugins: [solid({ hot: mode !== "test" })],

  // Component tests (`npm test`) render into a simulated DOM; nothing about
  // `tauri dev`/`tauri build` reads this section.
  test: {
    environment: "jsdom",
    setupFiles: ["./src/vitest-setup.ts"],
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
