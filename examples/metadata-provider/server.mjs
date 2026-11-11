#!/usr/bin/env node
/**
 * Minimal Ludex metadata provider (dependency-free).
 *
 * Implements the documented contract:
 *   GET /search?title=<text>&year=<optional>
 *   GET /games/{id}
 *
 * Responses use the canonical shape, so no field mapping is needed. Run it:
 *
 *   node examples/metadata-provider/server.mjs --port 8787
 *
 * Then configure a provider with base URL http://localhost:8787 in
 * Admin → Metadata providers. Loopback targets are refused unless the server
 * runs with LUDEX_PROVIDER_ALLOW_PRIVATE=true (tests/local only).
 */

import { createServer } from "node:http";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));

function arg(name, fallback) {
  const index = process.argv.indexOf(`--${name}`);
  if (index === -1) return fallback;
  return process.argv[index + 1] ?? fallback;
}

const port = Number(arg("port", "8787"));
const host = arg("host", "127.0.0.1");
// Optional: when set, requests must send `Authorization: Bearer <token>`.
const token = arg("token", "");
const fixturePath = arg("fixture", join(here, "fixtures", "games.json"));

/** @type {Array<Record<string, unknown>>} */
const games = JSON.parse(readFileSync(fixturePath, "utf8"));
if (!Array.isArray(games)) {
  console.error("fixture must be a JSON array of candidates");
  process.exit(1);
}

function normalise(value) {
  return String(value ?? "")
    .toLowerCase()
    .normalize("NFKD")
    .replace(/[\u0300-\u036f]/g, "")
    .replace(/[^a-z0-9]+/g, " ")
    .trim();
}

function search(title, year) {
  const query = normalise(title);
  const wanted = Number.isFinite(year) ? year : null;
  return games
    .map((game) => {
      const candidate = normalise(game.title);
      if (!query || !candidate.includes(query)) return null;
      let score = candidate === query ? 100 : 60;
      const releaseYear = Number(String(game.releaseDate ?? "").slice(0, 4));
      if (wanted && releaseYear === wanted) score += 40;
      return { game, score };
    })
    .filter(Boolean)
    .sort((left, right) => right.score - left.score)
    .slice(0, 20)
    .map((entry) => entry.game);
}

function send(response, status, body) {
  const payload = JSON.stringify(body);
  response.writeHead(status, {
    "content-type": "application/json; charset=utf-8",
    "content-length": Buffer.byteLength(payload),
    "cache-control": "no-store",
  });
  response.end(payload);
}

const server = createServer((request, response) => {
  const url = new URL(request.url ?? "/", `http://${request.headers.host ?? host}`);

  if (token && request.headers.authorization !== `Bearer ${token}`) {
    send(response, 401, { error: { code: "unauthorized", message: "Bearer token required" } });
    return;
  }

  if (request.method !== "GET") {
    send(response, 405, { error: { code: "method_not_allowed", message: "Use GET" } });
    return;
  }

  if (url.pathname === "/search") {
    const title = url.searchParams.get("title") ?? "";
    const year = Number(url.searchParams.get("year") ?? "");
    send(response, 200, { results: search(title, Number.isNaN(year) ? null : year) });
    return;
  }

  const match = /^\/games\/([^/]+)$/.exec(url.pathname);
  if (match) {
    const id = decodeURIComponent(match[1]);
    const game = games.find((entry) => String(entry.id) === id);
    if (!game) {
      send(response, 404, { error: { code: "not_found", message: "Unknown game id" } });
      return;
    }
    send(response, 200, game);
    return;
  }

  if (url.pathname === "/health") {
    send(response, 200, { status: "ok", games: games.length });
    return;
  }

  send(response, 404, { error: { code: "not_found", message: "Unknown route" } });
});

server.listen(port, host, () => {
  console.log(`metadata provider listening on http://${host}:${port} (${games.length} fixtures)`);
  if (token) console.log("bearer token required");
});
