import { defineConfig, loadEnv, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

/**
 * Replaces `target` in one step: a crash or a second build never observes a
 * half-written file, and there is no check-then-write window (#266).
 */
function writeFileAtomic(target: string, contents: string): void {
  const temporary = `${target}.${process.pid}.tmp`;
  fs.writeFileSync(temporary, contents);
  fs.renameSync(temporary, target);
}

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
      let source: string;
      try {
        // No `existsSync` probe: the read itself reports a missing bundle,
        // which removes the check-then-write race (#266).
        source = fs.readFileSync(worker, "utf8");
      } catch {
        return;
      }

      // Locale catalogs are runtime-cached, never precached (#237): shipping
      // all 13 on first install downloaded ~4.5 MB of translations the user
      // cannot read. `sw.js` caches them on demand via the static-asset route.
      const manifest = JSON.parse(
        fs.readFileSync(path.resolve(import.meta.dirname, "src/locales/manifest.json"), "utf8"),
      ) as Array<{ code: string; file: string | null }>;
      const localeChunks = manifest
        .filter((entry) => entry.code !== "en" && entry.file)
        .map((entry) => `${entry.code}-`);

      const files = walkFiles(dist).sort();
      const precache = files
        .filter((file) => file !== "sw.js")
        .filter((file) => !localeChunks.some((prefix) => file.startsWith(`assets/${prefix}`)))
        .map((file) => `./${file}`);
      const hash = createHash("sha256");
      for (const file of files) {
        hash.update(file);
        hash.update(fs.readFileSync(path.join(dist, file)));
      }
      const version = hash.digest("hex").slice(0, 12);

      source = source.replace('const VERSION = "__CACHE_VERSION__";', `const VERSION = "${version}";`);
      source = source.replace("const PRECACHE = __PRECACHE__;", `const PRECACHE = ${JSON.stringify(precache, null, 2)};`);
      writeFileAtomic(worker, source);
      this.info?.(`service worker: ${precache.length} precache entries, version ${version}`);
    },
  };
}

/**
 * Vendor grouping (#237). Keeping React, the router, the query cache, icons
 * and motion in their own chunks lets the browser reuse them across the lazy
 * route chunks and keeps the entry below the single-chunk warning budget.
 */
function manualChunks(id: string): string | undefined {
  if (!id.includes("node_modules")) return undefined;
  if (id.includes("react-router")) return "vendor-router";
  if (id.includes("@tanstack")) return "vendor-query";
  if (id.includes("lucide-react")) return "vendor-icons";
  if (id.includes("motion")) return "vendor-motion";
  if (id.includes("@tauri-apps")) return "vendor-tauri";
  if (/[\\/]node_modules[\\/](react|react-dom|scheduler)[\\/]/.test(id)) return "vendor-react";
  return "vendor";
}

// base: "./" keeps every asset relative so the same bundle works inside the
// Tauri shell and when the Rust server embeds dist/ under any path.
export default defineConfig(({ command, mode }) => {
  const env = loadEnv(mode, import.meta.dirname, "");
  const apiUrl = env.VITE_API_URL || "http://localhost:8080";
  // Compile-time flag: the bundler folds every mock branch, so the demo
  // library never reaches a production bundle (#237). Dev keeps the mocks.
  const usingMocks = env.VITE_USE_MOCKS === "true" || (env.VITE_USE_MOCKS === undefined && command === "serve");
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
      __USING_MOCKS__: JSON.stringify(usingMocks),
    },
    resolve: {
      alias: [
        // Production builds never load the demo adapters (#237): alias the two
        // entry points to tiny stubs so `lib/mock.ts`, `lib/mock-install.ts`
        // and `data/demo.ts` are not even in the module graph. Mock and demo
        // builds (`VITE_USE_MOCKS=true` or `vite dev`) use the real modules.
        ...(usingMocks
          ? []
          : [
              {
                find: /^@\/lib\/mock$/,
                replacement: fileURLToPath(new URL("./src/lib/mock-disabled.ts", import.meta.url)),
              },
              {
                find: /^@\/lib\/mock-install$/,
                replacement: fileURLToPath(new URL("./src/lib/mock-install-disabled.ts", import.meta.url)),
              },
            ]),
        { find: /^@\//, replacement: `${fileURLToPath(new URL("./src", import.meta.url))}/` },
      ],
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
      rolldownOptions: {
        output: {
          manualChunks,
        },
      },
    },
  };
});
