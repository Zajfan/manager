import { defineConfig } from "vite";
import solid from "vite-plugin-solid";
// @ts-expect-error type error without @types/node package
import process from "node:process";
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [solid()],

  // Component tests (`npm test`) render into a simulated DOM; nothing about
  // `tauri dev`/`tauri build` reads this section.
  test: {
    environment: "jsdom",
    setupFiles: ["./src/vitest-setup.ts"],
    // Without this, Vitest transforms .tsx in SSR mode, so vite-plugin-solid's
    // HMR transform emits an import of its "/@solid-refresh" virtual module —
    // meaningless outside a real Vite dev server, and on Windows it isn't
    // even a parseable path, crashing before a single test runs. Forcing the
    // "web" transform for jsx/tsx is vite-plugin-solid's own documented
    // Vitest setup (https://github.com/solidjs/vite-plugin-solid#testing).
    transformMode: {
      web: [/\.[jt]sx?$/],
    },
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
