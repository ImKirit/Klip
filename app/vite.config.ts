/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Port 1430 is the dev URL of the Tauri app. In a plain browser the UI talks to the
// test backend (klip-devserver) through the proxy below, same origin, no CORS.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1430,
    strictPort: true,
    host: "127.0.0.1",
    // The keyboard layouts are imported from the shared crate, one folder up.
    // The Rust build and the test clips change constantly and are none of Vite's business.
    watch: { ignored: ["**/src-tauri/**", "**/testdata/**"] },
    fs: { allow: [".", "../crates/klip-input/layouts"] },
    proxy: {
      "/__klip": { target: "http://127.0.0.1:1431", changeOrigin: false },
    },
  },
  build: {
    target: "chrome120",
    sourcemap: false,
  },
  test: {
    include: ["src/**/*.test.ts"],
  },
});
