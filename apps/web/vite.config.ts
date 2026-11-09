import { defineConfig, loadEnv } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { fileURLToPath } from "node:url";

// base: "./" keeps every asset relative so the same bundle works inside the
// Tauri shell and when the Rust server embeds dist/ under any path.
export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, import.meta.dirname, "");
  const apiUrl = env.VITE_API_URL || "http://localhost:8080";

  return {
    base: "./",
    plugins: [react(), tailwindcss()],
    resolve: {
      alias: {
        "@": fileURLToPath(new URL("./src", import.meta.url)),
      },
    },
    server: {
      port: 5173,
      strictPort: false,
      proxy: env.VITE_API_URL
        ? undefined
        : {
            "/api": {
              target: apiUrl,
              changeOrigin: true,
            },
          },
    },
    build: {
      target: "es2022",
      outDir: "dist",
      emptyOutDir: true,
      sourcemap: false,
    },
  };
});
