// Vite build of the desktop UI. The Tauri CLI runs the dev server and embeds the build
// output; the settings follow the Tauri guide linked below.
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// https://v2.tauri.app/start/frontend/vite/
// NB: the port MUST match devUrl in src-tauri/tauri.conf.json, hence strictPort
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 5173, strictPort: true },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: { target: "es2022" },
});
