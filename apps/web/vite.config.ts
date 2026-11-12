import { defineConfig, loadEnv, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

function walkFiles(dir: string, root = dir): string[] {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) return walkFiles(full, root);
    return [path.relative(root, full).split(path.sep).join("/")];
  });
}

/**
 * Injects the built asset list and a content hash into `dist/sw.js`, so the
 * shell precache is versioned per build and old caches are dropped on activate.
 * No external PWA plugin: the surface we need is small and auditable.
 */
function serviceWorkerPlugin(): Plugin {
  return {
    name: "ludex-service-worker",
    apply: "build",
    closeBundle() {
      const dist = path.resolve(import.meta.dirname, "dist");
      const worker = path.join(dist, "sw.js");
      if (!fs.existsSync(worker)) return;

      const files = walkFiles(dist).sort();
      const precache = files.filter((file) => file !== "sw.js").map((file) => `./${file}`);
      const hash = createHash("sha256");
      for (const file of files) {
        hash.update(file);
        hash.update(fs.readFileSync(path.join(dist, file)));
      }
      const version = hash.digest("hex").slice(0, 12);

      let source = fs.readFileSync(worker, "utf8");
      source = source.replace('const VERSION = "__CACHE_VERSION__";', `const VERSION = "${version}";`);
      source = source.replace("const PRECACHE = __PRECACHE__;", `const PRECACHE = ${JSON.stringify(precache, null, 2)};`);
      fs.writeFileSync(worker, source);
      this.info?.(`service worker: ${precache.length} precache entries, version ${version}`);
    },
  };
}

// base: "./" keeps every asset relative so the same bundle works inside the
// Tauri shell and when the Rust server embeds dist/ under any path.
export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, import.meta.dirname, "");
  const apiUrl = env.VITE_API_URL || "http://localhost:8080";
  // The web build version shown on the auth screens (#175). The server has its
  // own version endpoint; this one must match the bundle that renders it.
  const pkg = JSON.parse(
    fs.readFileSync(path.join(import.meta.dirname, "package.json"), "utf8"),
  ) as { version: string };

  return {
    base: "./",
    plugins: [react(), tailwindcss(), serviceWorkerPlugin()],
    define: {
      __APP_VERSION__: JSON.stringify(pkg.version),
    },
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
