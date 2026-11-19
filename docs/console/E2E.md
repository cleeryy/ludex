# Console end-to-end specs (#302)

The Console's input is a fake standard gamepad, not a mocked React hook. The
pad object is installed before the bundle, `navigator.getGamepads` returns it,
and the application's real polling loop decodes it with the real controller
profile. Every assertion therefore goes through the same code path a physical
pad would.

This page is the how-to for that harness. The evidence it produces is linked
from [evidence.md](evidence.md).

## What runs

| File | What it proves |
|---|---|
| `e2e/pad.ts` | The fake pad: 17 buttons, 4 axes, timestamp, hot-plug, A/B swap, frame discipline. |
| `e2e/native.ts` | A minimal Tauri bridge for the native-only flows (install, launch, power). |
| `e2e/tests/06-console.spec.ts` | Boot, deep link, Escape at the root, the raw pad path, the 1920×1080 evidence frame, overlay single-dispatch. |
| `e2e/tests/10-console-flows.spec.ts` | Home → launch plate → B; install → Status bar + Downloads; settings persistence; Power → confirm → Cancel. |
| `e2e/tests/07-console-visual.spec.ts` | Capture-only TV matrix: 720p/1080p/4K plus a 5 % overscan crop. |
| `e2e/tests/08-console-antislop.spec.ts` | The 24u type floor, Cabinet rules and desktop leaks, read from the rendered DOM. |
| `e2e/tests/09-console-perf.spec.ts` | Opt-in TV probe: boot, rail frames, long tasks, heap, rAF chains. |

`scripts/e2e.sh` is the harness: it builds the web UI and the server, starts an
isolated main server with an admin and one indexed archive, a second fresh
server for setup, and a local HTTP server for ingest. Playwright runs with one
worker because the suite shares those servers and the seeded library.

## The fake pad

```ts
const pad = await installFakePad(page);   // before page.goto
await page.goto(consoleUrl());
await pad.tap("a");                        // confirm: press, two frames, release
await pad.tap("b");                        // back
await pad.hold("right", 700);              // crosses the 380 ms repeat delay
await pad.chord(["lb", "rb"]);             // both shoulders in one frame
await pad.disconnect();                    // hot-unplug
await pad.connect();                       // hot-plug
```

- Install it **before** `page.goto`: `addInitScript` only applies to documents
  created afterwards.
- Every state change waits two animation frames, so the assertion cannot race
  the rAF poll. `tap`/`hold`/`chord` press and release inside the page, which
  keeps the press visible for at least two polls on a loaded CI runner.
- `pad.tap("a")` is the standard mapping's button 0. A Nintendo pad is
  `pad.setABSwap(true)`; the application reads the mapping, the spec does not
  swap its own inputs.
- `pad.state()` reads the pad the page sees; `pad.events()` is the hot-plug
  history. Use them when the assertion is about the harness, not the UI.

Keep one press per assertion. A spec that presses twice and then checks two
things cannot tell a swallowed press from a double dispatch; that is the exact
failure the overlay test exists to catch.

## The native bridge

The browser build is a companion: it browses and downloads, but `isNative()` is
false and installs, launches and sleeps do not exist. Three flows in #302 only
exist on the native path, so `e2e/native.ts` installs a small Tauri bridge
before the bundle:

```ts
const bridge = await installNativeBridge(page, {
  serverUrl: process.env.LUDEX_E2E_BASE_URL!,
  token: process.env.LUDEX_E2E_TOKEN!,
  installed: [{ gameId }],   // Play instead of Install
  holdLaunch: true,          // launch_game stays pending: the plate reaches "slow"
});
```

It answers `get_settings`, `console_mode`, `window_state`, `sleep_capability`,
`list_installed`, `list_install_jobs`, `install_game` and `launch_game`, and
implements the event plugin (`plugin:event|listen`) so `install://progress` and
the other Tauri listeners work. Every other command returns `null`, which the
app already treats as "older desktop". It records **command names only** — the
install command carries the session token in its arguments.

What it proves: the real `isNative()` branches, the real install dialog, the
real downloads provider and its progress listener, the real launch plate, the
real Power confirmation, and the real single input owner above all of them.

What it does not prove: a process spawn, a real archive on disk, a real OS
sleep or quit. Those are hardware checks in
[NATIVE-CHECKLIST.md](NATIVE-CHECKLIST.md); the browser spec asserts the visible
flow and says so in its comments.

## Add a spec

1. Put it in `e2e/tests/`, numbered like the rest (`11-...`). Import
   `test`/`expect` from `@playwright/test`.
2. Sign in before the app boots:
   ```ts
   test.beforeEach(async ({ page }) => {
     await signInAsAdmin(page);
   });
   ```
3. Install the pad (and the native bridge, if the flow is native-only) **before
   `page.goto`**.
4. Navigate with `consoleUrl("/settings/appearance")`, not a hand-built hash;
   the helper honours `LUDEX_E2E_CONSOLE_PATH`.
5. Wait for a real surface, not the skeleton: a `[data-bp-card='true']` or a
   `[data-bp-settings-row='true']`. The shell fades in; an assertion on an
   opacity-0 header is a flake.
6. Press once, then assert the consequence: the surface, the active row, the
   focused card (`[data-gamepad-focus='true']`), the route, or the recorded
   native command.
7. Wrap the page in `watchConsole(page)` and call `errors.assertClean()` at the
   end. A React error must fail the spec, not scroll past in the log.

```ts
test("the Room row cycles from the pad", async ({ page }) => {
  const errors = watchConsole(page);
  const pad = await installFakePad(page);
  await page.goto(consoleUrl("/settings/appearance"));
  await expect(page.getByRole("heading", { name: "Settings", exact: true })).toBeVisible();

  const room = page.getByRole("button", { name: /^Room: / });
  await expect(room).toHaveAttribute("aria-label", "Room: Night");
  await pad.tap("down");                    // composition -> room
  await expect(room).toHaveAttribute("data-system-active", "true");
  await pad.tap("a");
  await expect(room).toHaveAttribute("aria-label", "Room: Projector");

  errors.assertClean();
});
```

A flow that needs more than a few seconds (the launch plate's 8 s line) sets
`test.setTimeout(60_000)` — the default is already 60 s in
`e2e/playwright.config.ts`; do not shorten it with fake timers, because the
input loop is real.

## Run it

```bash
# Full harness: builds web + server, starts the fixtures, runs every spec.
scripts/e2e.sh

# Reuse an already-built server binary and a running harness:
LUDEX_E2E_BIN=target/debug/ludex-server scripts/e2e.sh
pnpm --filter @ludex/e2e exec playwright test tests/10-console-flows.spec.ts

# One spec against a harness whose env is already exported:
source <harness env>   # LUDEX_E2E_BASE_URL, LUDEX_E2E_TOKEN, LUDEX_E2E_GAMES_DIR
pnpm --filter @ludex/e2e exec playwright test tests/10-console-flows.spec.ts
```

`LUDEX_E2E_CONSOLE_PATH` overrides the shell path (default `/console`);
`/bigpicture` is only a redirect since #267.

## Debug a failure

- `?gamepadDebug=1` on the console URL renders the diagnostics panel the input
  loop writes: the pad id, decoded action, profile and raw axes.
- `?motionDebug=1` renders the motion probe: which transitions ran and how long
  they took.
- Playwright keeps a trace on failure (`trace: retain-on-failure`) under
  `e2e/test-results/`; open it with `pnpm e2e:report`.
- `e2e/test-results/console-visual-gate/` holds the red-on-grey diff when the
  visual gate fails.

## Rules

- No mocked input hooks. If a test needs a different pad, configure the fake
  pad; do not stub `useGamepad`.
- No second input owner. A spec that installs two providers is testing a bug.
- A browser assertion never claims a process started. Say what the browser can
  see, and point the hardware half at `NATIVE-CHECKLIST.md`.
- Keep the console error-free: `watchConsole` is part of the assertion, not
  decoration.
