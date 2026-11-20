# Launch plans, auto-revert and diagnostics

A **launch plan** is everything that decides how a game starts on this
machine: the launch mode (`auto`, `native`, `proton`, `umu`), the compatibility
tool id, the Wine prefix, the environment overrides, the winetricks verbs, the
Steam app id, the working directory, the display profile and the executable
arguments. Ludex stores the per-game options in the desktop index, resolves
them into a command at launch time, and writes a header plus the wrapper output
to `launch-<gameId>-<sessionId>.log` (#280).

The rules engine (#297) and the settings tiers (#299) are separate features.
This document covers what happens after a launch fails: how Ludex notices,
when it puts a working setup back, and what the diagnostics bundle contains
(#298).

## Failure detection

A launch is a **failure** when either is true:

| Evidence | Meaning |
|---|---|
| The process tree ended inside the **30 s** startup budget | no first frame was painted before the wrapper or the game exited |
| The wrapper reported a known fatal exit: `53` / `0x35`, `0xC0000409` | these codes come from a launch that never reached a frame (see the exit-code table in [LINUX-GAMING.md](LINUX-GAMING.md#exit-codes)) |

The 30 s budget is the same one the focus watch uses, and the same one the
documented troubleshooting flow assumes. A session that ran **past** the budget
and exited without a fatal code is recorded as the last plan that worked; a
fatal exit is always a failed launch, because those codes are produced before a
frame in practice.

A launch the player stopped — `stop_game`, the tray, the overlay or Force close
— is **not counted** in either direction. A deliberate stop is neither a
failure nor proof that the plan works.

ROMs launched through an emulator (#52) have no compatibility plan and are
never counted.

## Auto-revert

Counters are **per profile**, not per game: the profile is the SHA-256 of the
options, executable and arguments. Changing one option (a different tool, a new
`DXVK_*` variable, another prefix) starts a new streak, so an old failure never
poisons a setup the user just changed.

The rules:

1. The first failure is counted and nothing is changed. The game page and the
   console show the existing immediate-exit message.
2. A **second consecutive failure with the same profile** reverts to the last
   plan that reached a frame. Ludex writes those options back to the game row,
   resets the streak and says it in one sentence: the game kept failing to
   start, so the last launch setup that worked is back, and the next launch
   uses it.
3. The **third launch** therefore runs the previous working plan. No extra
   click and no retry loop: a new process is never spawned automatically, so two
   `wineserver`s can never race over the same prefix.
4. A successful launch (past the budget) records the current plan as the last
   one that worked and clears the streak. A deliberate stop clears nothing.
5. With no plan known to work yet, there is nothing to revert to. The second
   failure is reported in the log (and in the bundle's `failures` block) but the
   options stay as they are; the streak stays at two and does not retrigger.

The state lives in `{appDataDir}/launch-recovery.json`, keyed by game id:
the last working plan, the last launched plan, the streak and the last failure
(time, exit code, seconds). It is plain local JSON, not a database table, so
there is no migration and deleting the file only forgets the counters. The
shell receives `game://recovery` once per revert, with the two profile hashes
and the sentence; the console shows the same sentence on its notice surface.
The event carries metadata only — the options themselves are already saved.

## Diagnostics bundle

One click produces
`{appDataDir}/diagnostics/launch-bundle-<gameId>-<stamp>.zip`:

| Entry | Contents |
|---|---|
| `manifest.json` | schema, app version, platform/arch, distro id and pretty name, kernel, GPU vendor + kernel driver + NVIDIA driver version, UMU/Proton/Wine/Gamescope versions, launch mode and tool, the resolved plan (program, arguments, environment, prefix, Gamescope line), prefix health, the last failure and the retained log names |
| `report.md` | the same metadata as a ready-to-paste issue body, including the plan hash |
| `logs/launch-*.log` | the newest retained per-launch compatibility logs for this game (at most five, each capped at its last 512 KiB per #280) |

Prefix health reports existence, the Proton `pfx` layout, `drive_c` and
`system.reg` (with its size), never the whole folder.

Where to produce it:

- **Desktop:** Game page → Launch options → **Diagnostics bundle**. The
  **Report this game** button then opens a GitHub new-issue URL prefilled with
  `report.md` — metadata only, no zip upload and no files.
- **Console:** Quick actions (Start/Guide) → **Diagnostics bundle**, for the
  selected game. The whole path is reachable with the pad; the notice shows the
  file name, and the bundle is written to the app data folder.

### What is redacted

The bundle is written through the same redactor as the local crash reports
(#171), plus one extra pass before serialization:

- environment values whose key names a credential (`TOKEN`, `PASS`, `SECRET`,
  `KEY`, `CREDENTIAL`, `AUTH`) become `<redacted>`;
- `Bearer` credentials, `token=…`-style values and long opaque strings become
  `<token>`;
- server URLs, e-mail addresses and IP addresses are replaced;
- the home directory becomes `~`, the account name `<user>`, the hostname
  `<host>`; the generic path scanner then reduces those paths to
  `<path:hash>`.

Game files, saves, snapshots, account tokens and passwords are never copied
into the bundle. The plan is identified by its SHA-256 — the `planHash` in the
result and in `report.md` — so two reports of the same profile correlate
without naming the user's folders. The plan hash is the one opaque value that
is deliberately kept after redaction.

### Reporting

The **Report this game** action only opens a URL. Nothing is uploaded
automatically: the user attaches the zip to their own issue if they want to.
The prefilled body is capped so the URL stays within browser limits and
contains no log lines by default.
