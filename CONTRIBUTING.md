# Contributing to Ludex

Thanks for helping. Ludex is a self-hosted game library for DRM-free files:
Rust server (`crates/`), React web UI (`apps/web`), Tauri desktop
(`apps/desktop`) and a Playwright E2E suite (`e2e/`).

Be direct, assume good faith, and keep reviews about the code. Harassment or
personal attacks are not welcome.

## License

Ludex v0.2.0 and later is licensed under **AGPL-3.0-or-later**. By submitting a
pull request you agree that your contribution is licensed under the same terms.
v0.1.0 was released under MIT OR Apache-2.0; that historical grant is not
revoked.

Do not paste code from GameVault or any other project with a different license
into Ludex. This repository is a clean-room implementation.

## Setup

Prerequisites: Rust stable, Node 22, pnpm 9. For the desktop app, install the
[Tauri 2 prerequisites](https://v2.tauri.app/start/prerequisites/) (WebKitGTK
4.1 and the appindicator/rsvg packages on Linux, WebView2 on Windows).

```bash
pnpm install
pnpm dev                     # web dev server
cargo run -p ludex-server    # API on 127.0.0.1:8080
scripts/dev.sh               # both, with .dev/{data,games,media}
```

## Branches and commits

- Branch from `main`; keep one topic per branch.
- Use [Conventional Commits](https://www.conventionalcommits.org/):
  `feat(server): ...`, `fix(web): ...`, `docs: ...`, `ci: ...`,
  `test(e2e): ...`, `chore(deps): ...`, `refactor: ...`.
- Imperative, lower-case subject, no trailing period. Explain *why* in the body
  when the change is not obvious.
- Do not force-push a branch under review unless the reviewer asks.
- Never commit secrets, tokens, `.env` files, database dumps, game archives,
  course PDFs or build output.

## Pull requests

1. Open a focused PR against `main`. Fill in the template and link the issue.
2. CI must be green. Branch protection requires:
   - `Rust (fmt, clippy, test)`
   - `Web (install, build, typecheck)`
   - `Desktop (cargo check)`
   - `Docker image (build only)`
   - `E2E`
3. Resolve every review conversation. Stale approvals are dismissed on new
   commits.
4. Keep the history linear (rebase, do not merge `main` into the branch).
5. Update `CHANGELOG.md` for user-visible changes and `docs/CONTRACT.md` for API
   changes. Add a migration if the schema changes.

The `Desktop smoke (xvfb)` job is advisory (`continue-on-error`) because a
headless WebKit window check can be flaky on shared runners; it is not a required
check.

## Checks

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets

pnpm --filter @ludex/web build
pnpm --filter @ludex/web exec tsc --noEmit

pnpm e2e:install          # once
scripts/e2e.sh            # browser E2E: builds web + debug server, isolated fixtures
```

`scripts/e2e.sh` is the same harness the `E2E` CI job and the release gate run.
It starts two temporary servers (one seeded, one fresh for setup) and a local
HTTP server for ingest. It never touches your real library.

For the desktop app:

```bash
pnpm desktop:dev
pnpm desktop:dist
xvfb-run -a scripts/desktop-smoke.sh   # after a debug build
```

## Database and API

- Migrations live in `crates/ludex-server/migrations/` and are forward-only.
  Add a new numbered migration; do not edit an applied one.
- API JSON is camelCase; errors use the envelope documented in
  `docs/CONTRACT.md`.
- Path traversal, archive extraction and ingest URL validation are security
  boundaries. Add tests for changes there.

## Releases

Releases are tag-driven and E2E-gated. See
[`docs/RELEASING.md`](docs/RELEASING.md). Do not tag or publish from a pull
request.

## Security

Do not open a public issue for a vulnerability. Use the private advisory flow in
[`SECURITY.md`](SECURITY.md).
