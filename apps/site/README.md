# `@ludex/site` — Ludex Landing Page & Documentation

Static Next.js App Router + [Fumadocs](https://fumadocs.dev) site deployed to GitHub Pages at `https://cleeryy.github.io/ludex/`.

## Source of truth

Root [`docs/`](../../docs/) is the **only** canonical location for documentation.

- Never hand-edit files under `apps/site/content/docs/` or `apps/site/public/images/`.
- Edit the Markdown files under root `docs/`, then run `pnpm --filter @ludex/site sync` to regenerate the Fumadocs MDX pages, sidebar `meta.json` trees, rewritten links and copied assets.
- CI runs `node apps/site/scripts/sync-docs.mjs` followed by `git diff --exit-code` so generated content cannot drift from `docs/`.

## Local commands

```bash
# Install workspace dependencies from repository root
pnpm install

# Sync docs from root docs/ into apps/site/content/docs/
pnpm --filter @ludex/site sync

# Run local dev server (defaults to BASE_PATH=/ludex -> http://localhost:3000/ludex/)
BASE_PATH="" pnpm --filter @ludex/site dev

# Build static export into apps/site/out/
pnpm --filter @ludex/site build

# Verify every internal link and asset in apps/site/out/
pnpm --filter @ludex/site check:links

# Run Playwright smoke checks against apps/site/out/ under /ludex/
node apps/site/scripts/smoke.mjs
```

## Custom domain & base path

The build reads `BASE_PATH` (default `/ludex`) and `SITE_URL` (default `https://cleeryy.github.io/ludex`).
To switch to a custom domain later, set `BASE_PATH=""` and `SITE_URL="https://ludex.example.com"` in `.github/workflows/pages.yml` and add a `CNAME` file under `apps/site/public/`.
