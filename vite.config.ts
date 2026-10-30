import react from "@vitejs/plugin-react";
import { readFileSync } from "fs";
import { resolve } from "path";
import { defineConfig } from "vite";

const appVersion = readFileSync(resolve(__dirname, "VERSION"), "utf-8").trim();

export default defineConfig(async () => {
  return {
    plugins: [react()],
    define: {
      "import.meta.env.VITE_APP_VERSION": JSON.stringify(appVersion),
    },
    clearScreen: false,
    server: {
      port: 1422,
      strictPort: true,
    },
    envPrefix: ["VITE_", "TAURI_"],
    build: {
      rollupOptions: {
        input: {
          main: resolve(__dirname, "index.html"),
        },
      },
    },
  };
});
