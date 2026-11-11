import { defineConfig } from "vite";
import { resolve } from "node:path";

// Zuko's sounds are synthesized at runtime (src/core/sound.ts), so there are
// no audio assets to serve or copy.
export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: "127.0.0.1",
    // Cargo writes and locks build artifacts while Tauri compiles on Windows.
    watch: { ignored: ["**/target/**"] },
  },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: {
    target: "chrome110",
    minify: "esbuild",
    sourcemap: false,
    emptyOutDir: true,
    rollupOptions: {
      input: {
        island: resolve(__dirname, "index.html"),
        settings: resolve(__dirname, "settings.html"),
      },
    },
  },
});
