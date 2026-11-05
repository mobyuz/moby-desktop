import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// Tauri loads the built bundle from `dist` and the dev server from a fixed port.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  // WebView2 is evergreen, so modern CSS ships as written. Nothing is inlined: the production CSP refuses data: fonts.
  build: { target: "es2022", cssTarget: "chrome130", assetsInlineLimit: 0, sourcemap: false },
  test: { include: ["tests/**/*.test.ts"] },
});
