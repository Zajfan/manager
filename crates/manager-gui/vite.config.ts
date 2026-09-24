import { defineConfig } from "vite";
import solid from "vite-plugin-solid";
// @ts-expect-error type error without @types/node package
import process from "node:process";
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(({ mode }) => ({
  plugins: [solid()],

  // Solid's package exports a separate build under the "browser" condition;
  // Vitest runs in Node, so without forcing that condition here it resolves
  // the dev/HMR build instead, which broke path handling on Windows CI
  // (https://github.com/solidjs/vite-plugin-solid#testing, "vitest" section).
  resolve: {
    conditions: mode === "test" ? ["browser"] : [],
  },

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
