import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// Tauri expects a fixed port and fails if it is taken; the host is never exposed beyond this computer.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: "127.0.0.1",
    watch: { ignored: ["**/src-tauri/**", "**/tools/**"] },
  },
  build: {
    target: ["safari15", "chrome110"],
    // Not dist/: the build empties its folder first, and dist/ holds the packaged installers.
    outDir: "frontend-dist",
    emptyOutDir: true,
    sourcemap: false,
    chunkSizeWarningLimit: 1200,
  },
  test: {
    environment: "node",
    include: ["src/**/*.test.ts"],
  },
});
