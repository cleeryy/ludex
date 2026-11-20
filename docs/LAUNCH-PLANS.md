# Launch plans: auto-configuration, auto-revert and diagnostics

A **launch plan** is everything that decides how a game starts on this
machine: the launch mode (`auto`, `native`, `proton`, `umu`), the compatibility
tool id, the Wine prefix, the environment overrides, the winetricks verbs, the
Steam app id, the working directory, the display profile and the executable
arguments. Ludex stores the per-game options in the desktop index, resolves
them into a command at launch time, and writes a header plus the wrapper output
to `launch-<gameId>-<sessionId>.log` (#280).

Two features sit on top of that record:

- the **rules engine** (#297) evaluates the game and the machine, produces a
  plan without asking the player, and applies it at launch;
- the **auto-revert** (#298) counts failed launches per profile and puts the
  last working plan back instead of looping. A diagnostics bundle accompanies
  it.

On a console the mode is a consequence of the game, not a setting: the plan is
stated in one sentence ("Launching with UMU and GE-Proton, HDR off, GameMode
on, GAMEID 1145360."), stored per game, written into the launch log and
exported as JSON. Manual launch options stay available as the advanced tier and
always win over the plan.

Related: [LINUX-GAMING.md](LINUX-GAMING.md) (modes, AppImage environment, launch
logs, exit codes), [GAMESCOPE.md](GAMESCOPE.md) (display profiles),
[HDR-VRR.md](HDR-VRR.md), [SHADERS.md](SHADERS.md) (shader warm start) and
[CONTRACT.md](CONTRACT.md) (Tauri commands and events).

## Pipeline

```
installed game + saved options + host scan
        │
        ▼
PlanInput ── fingerprinted ──► stored plan (reused when the input matches)
        │
        ▼
RulesProvider::rules()  ──►  evaluate()  ──►  LaunchPlan (+ sentence)
        │                                        │
        │                                        ├─► prerequisite recipe (winetricks, gated by a prefix marker)
        │                                        ├─► LaunchOptions defaults (mode, tool, env, display, GameMode)
        │                                        └─► launch log section
        ▼
prepare_launch ──► process ──► #298 classifier ──► game://recovery / launch://plan-failed
```

Everything above the process boundary is pure: `evaluate`, `sentence`,
`apply_to_options`, `pending_prerequisites` and `PlanFailure::from_recovery`
are unit tested without Steam, Proton or a real game.

## Inputs

`PlanInput` is serialised and hashed (SHA-256, 12 bytes) as the plan
fingerprint. A stored plan is reused verbatim while the fingerprint matches; a
driver, engine, tool or option change makes it stale and re-evaluates it.

| Input | Source | Used for |
|---|---|---|
| `windowsExecutable`, `exeMachine` | PE header of the recorded executable, or the `.exe` suffix | UMU vs native runtime |
| `engine` | marker scan of the install folder (bounded: 3 levels, 400 entries): `UnityPlayer.dll`, `.uproject`/`Engine/`+`Binaries/`, `.pck`/`project.godot` | shader-compilation budget |
| `knownGameId` | the per-game Steam app id, passed to UMU as `GAMEID` | protonfixes and prefix layout |
| `archiveOrigin` | `ludex` install, `adopted` folder or launcher import (`launchUri`) | launcher entries skip the Wine layer |
| `gpu` | `/sys/class/drm/card*/device/{vendor,uevent}` | vendor/driver caveats (Nouveau) |
| `gamemode`, `mangohud` | `gamemoderun`/`mangohud` on `PATH` | wrappers (the shipped rules enable GameMode only; MangoHud stays off until a provider asks for it) |
| `winetricks` | compat scan | prerequisite recipe availability |
| `declaredPrerequisites` | redistributables found in the install folder (`vcrun`, `directx`, `dotnet`, `physx`) | automatic recipe |
| `display` | the stored per-game display profile | Gamescope/HDR/resolution |
| `manualOverride` | saved mode, tool, prefix, env or verbs | precedence |
| `tools` | compat scan (`proton:` ids and names) | Proton build preference resolution |

The engine split between Unreal 4 and 5 is not reliable from packaged markers,
so the scan reports Unreal generically; a data provider can refine it.

## Shipped rules

Deliberately few: each one is a general decision that can be justified without
a per-title database. The rules-database lane (#301) implements the same
`RulesProvider` trait with curated rows; it does not change the engine.

| Id | Matches | Plan effect |
|---|---|---|
| `launcher-uri` | launcher import with a `launchUri` | runtime `launcher-uri`, no Proton |
| `native-build` | non-PE executable | runtime `native`, no Proton |
| `windows-exe-umu` | Windows `.exe` from a Ludex install or an adopted folder | runtime `umu`, Proton pinned to GE-Proton |
| `steam-game-id` | Windows game with a known Steam app id | `GAMEID=<appId>` |
| `declared-vcrun` | install ships a `vcredist`/`vc_redist` | recipe `vcrun` |
| `declared-directx` | install ships `DXSETUP`/`directx` | recipe `directx` |
| `engine-shader-budget` | Unreal or Unity | expected startup budget 15 s + note |
| `gamemode-if-installed` | Windows game, `gamemoderun` present | wrapper GameMode |
| `nouveau-no-vulkan` | NVIDIA + `nouveau` | note: no usable Vulkan; install the proprietary driver |

Rules are sorted by ascending `priority` (provider order breaks ties) and later
rules win per field. A rule can only set fields it names; it never removes a
value another rule set.

## Plan and sentence

`LaunchPlan` (camelCase JSON) carries `runtime`, `executableKnown`, `proton`,
`toolId`, `gameIdEnv`, `env`, `prefix`, `prerequisites` (id + label + internal
verbs), `wrappers`, `display`, `firstFrameBudgetSecs`, `notes`, the matched
`rules`, `rulesVersion`, `source` (`auto`/`manual`) and `sentence`.

The sentence is composed from the plan, not from the user's options:

```
Launching with UMU and GE-Proton, HDR off, GameMode on, GAMEID 1145360.
Launching with UMU and GE-Proton, 1920x1080 at 60 Hz through Gamescope, HDR off, GameMode on.
Launching the native build, HDR off, GameMode off.
Launching with UMU and GE-Proton, HDR off, GameMode on, installing Visual C++ 2015–2022 runtime first.
```

HDR is off unless the stored display profile asks for it (a Gamescope profile is
a manual display choice, not a compatibility override). A resolution appears
only when Gamescope is actually requested; the plan never invents a mode.

When no executable is recorded yet (an adopted folder that resolves its launch
target only at play time), `executableKnown` is false: the sentence says
"Launching with the best available runtime" and the plan leaves the mode and the
tool to the existing auto rule instead of forcing one.

## Precedence: the manual override

`source: "auto"` means the plan is applied. Saving any of these per-game
options turns the plan informational (`source: "manual"`):

- launch mode other than `auto`
- a compatibility tool id
- a custom prefix path
- environment overrides
- winetricks verbs

Display profile, working directory and the Steam app id are inputs, not
overrides. When the plan is applied it only fills fields still on their
default: mode, tool id (when the plan resolves one), a default display profile
and missing environment keys. A manual value for one field does not disable the
rest of the plan.

GameMode is carried as an in-memory flag on the launch options and never
persisted, so the recovery profile hash (#298) only ever describes the user's
own values.

An auto-revert (#298) writes the last working profile back to the saved
options. That profile is then an explicit choice: the plan panel shows
**Manual override** until the user clears the options, at which point the rules
take over again. This is intentional — a revert restores a known-good launch,
not a rule decision.

## Prerequisites

A rule adds a stable recipe id; `recipe_for` maps ids to a human label and the
internal verbs:

| Id | Label | Verbs |
|---|---|---|
| `vcrun` | Visual C++ 2015–2022 runtime | `vcrun2019` |
| `directx` | DirectX 9 runtime | `d3dx9` |

`dotnet` and `physx` have no automatic recipe: the first has no single verb
that covers the runtimes, the second ships only from NVIDIA. They remain
covered by "Make it run".

Recipes are applied **before** the game starts, exactly once per prefix:

- gate: `<prefix>/.ludex/prerequisites.json` stores the recipe fingerprint,
  applied ids and failed ids. A different fingerprint (rules update) re-runs the
  recipe instead of assuming a partially applied prefix is complete;
- progress: `launch://plan-progress` per verb (`index`/`total`/`percent`) with
  human text such as "Installing Visual C++ 2015–2022 runtime (1/2)". No verb
  appears in the primary UI;
- cancel: `launch_plan_cancel` aborts between verbs and refuses to start the
  game. A running verb is allowed to finish first;
- failure policy: a missing `winetricks` or a failing verb is reported and the
  launch continues, because blocking Play on a package-manager problem is
  worse than starting the game. A failed recipe id is not marked applied, so
  the next launch retries it; a missing `winetricks` never writes the marker.

## Storage and export

```
<appDataDir>/launch-plans/<gameId>.json
```

```json
{
  "version": 1,
  "fingerprint": "…",
  "savedAt": "2026-11-19T10:00:00+00:00",
  "plan": { "sentence": "…", "rules": ["windows-exe-umu", "…"], "…": "…" },
  "lastFailure": { "kind": "no-first-frame", "exitCode": 53, "seconds": 2, "budgetSecs": 30, "hint": "…" },
  "lastFailureAt": "2026-11-19T10:04:00+00:00"
}
```

- `launch_plan_get`: reads the stored plan, or evaluates one without writing.
- `launch_plan_refresh`: evaluates and stores it (the "Recalculate" button).
- `launch_plan_export`: portable JSON (`kind: "ludex-launch-plan"`), no
  machine-specific paths; the web panel downloads it as a file.
- Deleting the file (or removing and relinking the game) simply re-evaluates on
  the next launch. Nothing in the plan rewrites the saved launch options.

The launch log records the same decision so support can reproduce the session
exactly. The header belongs to `prepare_launch` (#298), so the plan section is
appended right after it, before the wrapper writes its own output:

```
--- launch plan ---
plan: Launching with UMU and GE-Proton, HDR off, GameMode on, GAMEID 1145360.
plan runtime: umu · proton: GE-Proton · prefix: per-game · budget: 5s
plan rules: windows-exe-umu, steam-game-id, gamemode-if-installed (builtin-1)
plan source: auto
```

## Wrappers

- **Gamescope** comes from the stored display profile; the panel sentence names
  the resolution and refresh when one is set.
- **GameMode**: when the plan asks for it and `gamemoderun` is on `PATH`, the
  resolved entry point is wrapped as `gamemoderun <program> <args>`; nothing
  else in the command changes. The flag is never persisted.
- **MangoHud**: `MANGOHUD=1` is merged into the environment only when a rule
  asks for it, and never overrides a value the user saved.

## Failure detection (#298)

A launch is a **failure** when either is true:

| Evidence | Meaning |
|---|---|
| The process tree ended inside the **30 s** startup budget | no first frame was painted before the wrapper or the game exited |
| The wrapper reported a known fatal exit: `53` / `0x35`, `C0000409` | these codes come from a launch that never reached a frame (see the exit-code table in [LINUX-GAMING.md](LINUX-GAMING.md#exit-codes)) |

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

The plan's own `firstFrameBudgetSecs` (5 s, 15 s for Unreal/Unity) is the
expectation shown to support. It does **not** re-classify the session: the
#298 classifier is the only decision, and `launch://plan-failed` is emitted
from its failure arm.

## Auto-revert (#298)

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

### The plan-facing event

The same failure arm also tells the auto-configuration side, once:

- `launch://plan-failed` with `{ gameId, sessionId, planSentence, rules,
  failure }`, where `failure` is `{ kind, exitCode, seconds, budgetSecs, hint }`
  built from the #298 decision (`kind` is `fatal-exit` when
  `launch_recovery::is_fatal_exit` matched, `no-first-frame` otherwise;
  `budgetSecs` is the 30 s recovery budget). There is exactly one classifier
  and one counter update: the event never re-runs either;
- the failure is stored as `lastFailure` in the plan JSON, and the game page
  shows the hint. A session that runs past the budget clears it.

## Diagnostics bundle (#298)

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
is deliberately kept after redaction. The auto-config plan's `lastFailure`
block is written next to its stored plan, not into the bundle: the bundle reads
the recovery store.

### Reporting

The **Report this game** action only opens a URL. Nothing is uploaded
automatically: the user attaches the zip to their own issue if they want to.
The prefilled body is capped so the URL stays within browser limits and
contains no log lines by default.

## What this feature does not do

- no settings-tier redesign (#299)
- no TOML profile format (#300)
- no rules data file or curated title database (#301)
- no automatic change of the saved launch options by the rules engine: the plan
  is recomputed per launch and only applied in memory. The only writer of saved
  options is the #298 auto-revert, which restores a profile that already worked.

## Tests

`apps/desktop/src-tauri/src/launch_plan.rs` covers:

| Test | What it proves |
|---|---|
| `a_windows_game_gets_umu_ge_proton_gamemode_and_the_steam_game_id` | engine evaluation, tool resolution, GAMEID, sentence |
| `a_native_linux_game_runs_without_a_compatibility_layer` | native plan and sentence |
| `an_unknown_executable_leaves_the_runtime_to_the_auto_rule` | no forced mode without executable metadata |
| `an_imported_launcher_entry_keeps_its_uri` | launcher origin |
| `engine_markers_are_detected_from_the_install_folder` | Unity/Unreal/Godot fixtures |
| `the_engine_rule_raises_the_first_frame_budget` | engine → budget |
| `gpu_sysfs_fixture_produces_the_vendor_and_driver_profile` | GPU fixtures, Nouveau note |
| `a_gamescope_profile_puts_the_resolution_in_the_sentence` | display sentence |
| `a_declared_redistributable_becomes_a_named_prerequisite` | recipe from a declared redist |
| `a_custom_rules_provider_can_replace_the_plan` | `RulesProvider` seam (#301) |
| `manual_options_win_over_every_plan_field`, `the_plan_only_fills_fields_still_on_their_default` | override precedence |
| `a_plan_round_trips_through_the_store_and_goes_stale_with_its_input`, `a_stored_plan_is_reused_verbatim_while_the_inputs_match` | persistence and fingerprint |
| `prerequisites_run_once_per_recipe_and_restart_when_the_recipe_changes`, `the_marker_round_trips_inside_the_prefix` | recipe gating |
| `a_failure_is_described_from_the_recovery_decision` | the plan-facing description follows #298 |
| `gamemoderun_wraps_the_entry_point_only_when_asked` | wrapper command |
| `the_plan_is_written_into_the_launch_log`, `the_plan_section_is_appended_to_a_prepared_launch_log` | support reproduction |

No test starts a game, Proton or winetricks. The #298 classifier, revert rules
and bundle redaction have their own tests in `launch_recovery.rs` and
`launch_bundle.rs`.
