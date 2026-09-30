import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      // Rust build output: linked exes lock on Windows and crash the watcher.
      ignored: ["**/src-tauri/**", "**/target/**", "**/node_modules/**", "**/.git/**"],
    },
  },
  build: {
    outDir: "dist",
  },
});
