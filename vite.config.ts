import preact from "@preact/preset-vite";
import { defineConfig } from "vite";

// Tauri serves the built files itself; in development it loads this server.
export default defineConfig({
  plugins: [preact()],
  clearScreen: false,
  server: { port: 1420, strictPort: true, host: "127.0.0.1" },
  build: { target: "es2022", outDir: "dist", emptyOutDir: true },
});
