/* Ludex service worker.
 *
 * The build injects __CACHE_VERSION__ and __PRECACHE__ into this file, so the
 * shell cache is content-versioned and old caches are deleted on activate.
 *
 * Rules:
 *  - never cache API responses or auth, the one exception is the public
 *    /api/media/ cache (posters, backgrounds, icons);
 *  - navigation is network-first with the cached shell as fallback;
 *  - static build assets are cache-first;
 *  - a waiting worker only activates when the user clicks "Reload now".
 */

const VERSION = "__CACHE_VERSION__";
const PRECACHE = __PRECACHE__;

const SHELL_CACHE = `ludex-shell-${VERSION}`;
const STATIC_CACHE = `ludex-static-${VERSION}`;
const MEDIA_CACHE = "ludex-media-v1";
const MEDIA_LIMIT = 120;

const scope = self.registration.scope;
const urlFor = (path) => new URL(path, scope).href;
const INDEX_URL = urlFor("index.html");
const OFFLINE_URL = urlFor("offline.html");

self.addEventListener("install", (event) => {
  event.waitUntil(
    (async () => {
      const cache = await caches.open(SHELL_CACHE);
      // Add one by one: a single missing optional file must not fail install.
      await Promise.all(
        PRECACHE.map(async (path) => {
          const url = urlFor(path);
          try {
            const response = await fetch(new Request(url, { cache: "reload" }));
            if (response.ok) await cache.put(url, response);
          } catch {
            /* offline during install: the runtime cache fills in later */
          }
        }),
      );
    })(),
  );
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    (async () => {
      const keep = new Set([SHELL_CACHE, STATIC_CACHE, MEDIA_CACHE]);
      for (const key of await caches.keys()) {
        if (key.startsWith("ludex-") && !keep.has(key)) await caches.delete(key);
      }
      await self.clients.claim();
    })(),
  );
});

self.addEventListener("message", (event) => {
  if (event.data && event.data.type === "SKIP_WAITING") void self.skipWaiting();
});

function isStaticAsset(pathname) {
  return (
    pathname.includes("/assets/") ||
    /\.(?:js|css|woff2?|ttf|otf|png|jpe?g|webp|avif|gif|svg|ico|json|webmanifest)$/i.test(pathname)
  );
}

async function cacheFirst(request, cacheName) {
  const cache = await caches.open(cacheName);
  // Precache lives in the shell cache; check every cache first so an offline
  // navigation can reuse the assets it just loaded from the shell.
  const hit = (await caches.match(request)) ?? (await cache.match(request));
  if (hit) return hit;
  try {
    const response = await fetch(request);
    if (response.ok && response.type === "basic") await cache.put(request, response.clone());
    return response;
  } catch {
    return new Response("", { status: 504, statusText: "Offline" });
  }
}

async function trimMedia(cache) {
  const keys = await cache.keys();
  if (keys.length <= MEDIA_LIMIT) return;
  for (const key of keys.slice(0, keys.length - MEDIA_LIMIT)) await cache.delete(key);
}

async function mediaFirst(request) {
  const cache = await caches.open(MEDIA_CACHE);
  const hit = await cache.match(request);
  if (hit) return hit;
  try {
    const response = await fetch(request);
    if (response.ok && response.type === "basic") {
      await cache.put(request, response.clone());
      await trimMedia(cache);
    }
    return response;
  } catch {
    return new Response("", { status: 504, statusText: "Offline" });
  }
}

async function navigate(request) {
  try {
    const response = await fetch(request);
    if (response.ok) {
      const cache = await caches.open(SHELL_CACHE);
      await cache.put(INDEX_URL, response.clone());
    }
    return response;
  } catch {
    const shell = await caches.match(INDEX_URL);
    if (shell) return shell;
    const offline = await caches.match(OFFLINE_URL);
    return offline ?? new Response("Offline", { status: 503, headers: { "Content-Type": "text/plain" } });
  }
}

self.addEventListener("fetch", (event) => {
  const request = event.request;
  if (request.method !== "GET") return;

  const url = new URL(request.url);
  if (url.origin !== self.location.origin) return;

  // Downloads and range requests must go straight to the server.
  if (request.headers.has("range")) return;

  // API and auth responses are never cached. The public media cache is the
  // only exception: those are immutable, cache-busted image files.
  if (url.pathname.includes("/api/")) {
    if (url.pathname.includes("/api/media/")) event.respondWith(mediaFirst(request));
    return;
  }

  if (request.mode === "navigate") {
    event.respondWith(navigate(request));
    return;
  }

  if (isStaticAsset(url.pathname)) event.respondWith(cacheFirst(request, STATIC_CACHE));
});
