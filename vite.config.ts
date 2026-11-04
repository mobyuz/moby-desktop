import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// Tauri loads the built bundle from `dist` and the dev server from a fixed port.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: { target: "es2022", sourcemap: false },
  test: { include: ["tests/**/*.test.ts"] },
});
