import js from "@eslint/js";
import { defineConfig } from "eslint/config";
import tseslint from "typescript-eslint";

export default defineConfig(
  { ignores: ["dist/", "src-tauri/target/", "src-tauri/gen/"] },
  js.configs.recommended,
  tseslint.configs.recommended,
);
