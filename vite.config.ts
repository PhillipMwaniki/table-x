import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig({
  plugins: [react(), tailwindcss()],

  // Tauri expects a fixed port and fails if it is not available.
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: {
      // The Rust side is rebuilt by cargo, not Vite.
      ignored: ["**/src-tauri/**", "**/target/**", "**/crates/**"],
    },
  },

  // Produce output the Tauri bundler can consume.
  build: {
    // Safari 13 / Chromium 105 are the floor for Tauri's webviews.
    target: process.env.TAURI_ENV_PLATFORM === "windows" ? "chrome105" : "safari13",
    // Vite 8 builds on Rolldown and no longer ships esbuild; "oxc" is the
    // built-in minifier. Asking for "esbuild" here fails at build time with a
    // missing-package error rather than falling back.
    minify: process.env.TAURI_ENV_DEBUG ? false : "oxc",
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
  },

  resolve: {
    alias: {
      "@": new URL("./src", import.meta.url).pathname,
    },
  },
});
