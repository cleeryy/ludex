# @ludex/web

The Ludex interface. One React bundle runs in the browser (served by `ludex-server`) and inside
the Tauri desktop shell, where it gains install, launch and playtime through `src/lib/native.ts`.

Design direction, tokens and component rules: [`DESIGN.md`](./DESIGN.md).

## Run it

```bash
pnpm install
pnpm --filter @ludex/web dev        # http://localhost:5173
```

The dev server proxies `/api` to `http://localhost:8080` unless `VITE_API_URL` is set.

```bash
pnpm --filter @ludex/web check      # tsc --noEmit
pnpm --filter @ludex/web build      # typecheck + static build in dist/
```

`dist/` uses relative asset paths (`base: "./"`) and hash routing, so the same output works at the
root of a server, under a sub-path, and from `tauri://localhost`.

## Environment

| Variable | Default | Meaning |
|---|---|---|
| `VITE_API_URL` | `""` (same origin) | API base. Set to `http://host:8080` when the UI is served from elsewhere. |
| `VITE_USE_MOCKS` | on in dev, off in prod | `true` forces the mock adapter, `false` forces the real API. |

### Mock adapter

`src/lib/mock.ts` implements the whole contract in memory: 18 games, versions, progress, bookmarks,
users, news, ingest jobs whose progress advances, a scanner that finishes, and realistic errors.
Passwords for demo accounts use `ludex` (`mara` is the admin, `jonas` the editor).

Two review switches live in localStorage:

```js
localStorage.setItem("ludex.demo.needsSetup", "1"); // first-run server with no users
localStorage.setItem("ludex.demo.empty", "1");      // empty library
```

## Structure

```
src/
  app.tsx                 providers, routing, boot gate
  lib/
    api.ts                the only network surface + assetUrl() + token store
    mock.ts               mock adapter (dev/demo)
    native.ts             Tauri bridge, every contract command, browser fallbacks
    art.ts                palettes, motifs, deterministic art for missing covers
    types.ts              docs/CONTRACT.md, verbatim
    hooks.ts              library queries, bookmarks, launch, installed games, Tauri events
    format.ts             bytes, minutes, dates, ratings
  components/
    art/cover.tsx         generated covers, backdrops, stills
    ui/                   button, input, dialog, popover, select, toast, …
    shell/                app frame, rail, command palette
    game/                 card, state marks
  screens/                setup, login, library, detail, news, settings, 404, admin/*
  styles/index.css        tokens, base layer, component classes
```

## Native bridge

`native.ts` detects `window.__TAURI_INTERNALS__`, then dynamic-imports `@tauri-apps/api/core` and
`@tauri-apps/api/event`. Every command in `docs/CONTRACT.md` is wrapped and typed. In a browser:

- reads return empty (`listInstalled` → `[]`, `getSettings` → `null`),
- actions throw a friendly error (“That needs the Ludex desktop app…”),
- the UI shows **Download** instead of **Install/Play**, and the setup wizard skips local mode.

Media never uses a bare relative URL. Server responses are origin-relative, so everything goes
through `assetUrl(path)` in `api.ts`, which prefixes the configured base (the remote URL in Tauri,
empty for same-origin in a browser).

## Review screenshots

`screenshots/` holds 1440×900 captures of the main screens (library grid and list, detail in both
themes, ingest, scan, users, news editor, server, command palette, setup, login, empty library,
mobile library and detail). They are generated from the mock adapter.

## Known gaps

- The demo library has no trailer assets, so the media strip shows stills only. Trailer tiles and
  the video dialog are implemented and appear when the API returns a trailer URL.
- Local mode in the setup wizard calls `local_server_start`; it needs the desktop shell to run.
  Browser setup creates the server administrator through `POST /api/setup`.
- Library pagination is capped at 200 items per request (the contract default is 48; the UI asks
  for more because the demo library is small). A “load more” control is not built yet.
