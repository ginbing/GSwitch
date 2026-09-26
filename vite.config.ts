import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig(({ mode }) => ({
  plugins: [react(), tailwindcss()],
  resolve: mode === "demo" ? {
    alias: [
      { find: /^\.\/api$/, replacement: demoPath("./demo/api.ts") },
      { find: /^\.\/updater$/, replacement: demoPath("./demo/updater.ts") },
    ],
  } : undefined,
  publicDir: "assets",
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
}));

function demoPath(path: string) {
  return decodeURIComponent(new URL(path, import.meta.url).pathname).replace(/^\/([A-Za-z]:)/, "$1");
}
