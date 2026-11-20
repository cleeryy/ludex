# Compatibility rules (#301)

Ludex ships a small database of launch rules. A rule matches facts the app
already knows about a game and this machine, then fills in launch defaults with
a reason a human can read. The database is data, not code: it ships with the
app, updates independently through a signed download, and every applied rule is
visible in the game's "why" panel.

This is the format and validation contract. The launch consumer is #297; the
three-tier settings surface is #299. Neither is implemented here.

## Files

| Path | Role |
| --- | --- |
| `data/compat-rules.toml` | Shipped database, embedded in the binary at build time. |
| `data/compat-rules.toml.minisig` | Detached minisign signature served next to the file. |
| `{appDataDir}/compat-rules.toml` | Signed cache from the last successful update. |
| `{appDataDir}/compat-rules.toml.minisig` | Signature checked before the cache is used. |
| `{appDataDir}/compat-rules.local.toml` | Local overrides ("learned locally"). |
| `{appDataDir}/compat-rules.state.json` | Last check time, fetched version, last error. |

`apps/desktop/src-tauri/src/compat_rules.rs` owns parsing, validation, matching
and application. It has no launch-path side effects: `load` reads files and
returns a set; `evaluate` is pure.

## Schema

```toml
schema = 1                 # required; this build understands 1
version = "2026.11"        # required; YYYY.MM or YYYY.MM.N, shown as "rule 2026.11"
updated = "2026-11-19"     # optional ISO date
description = "…"          # optional; not shown in the UI

[[rules]]
id = "linux-windows-exe-needs-a-compat-tool"   # required, unique slug
reason = "…"                                     # required, the sentence the panel shows
confidence = "certain"                           # certain | likely | guess
source = "Ludex launch model (…)"                # required for shipped rules
prerequisites = ["vcrun"]                        # vcrun | dotnet | directx | physx

[rules.match]
platform = "linux"          # linux | windows | macos
executable = "windows"      # native | windows | windows-arm64 | unknown
# Optional, all ANDed:
# game_id = 42              # local rules only; server ids are not portable
# exe_sha256 = "…"          # 64 lowercase hex characters
# engine = "unity"          # [a-z0-9][a-z0-9._-]{0,31}, case-insensitive compare
# gpu_vendor = "nvidia"     # nvidia | amd | intel | unknown
# arch = "x86_64"           # x86_64 | aarch64 | x86

[rules.plan]                # optional, but the rule must do something
mode = "umu"                # auto | native | proton | umu
args = ["-windowed"]        # appended after the user's own arguments (#297)
winetricks = ["mfplat"]     # stored as the per-game verb list
steam_app_id = 480

[rules.plan.env]
PROTON_ENABLE_NVAPI = "1"   # same validator as the UI; reserved variables refused

[rules.plan.display]        # a complete Gamescope profile, or nothing
mode = "gamescope"
width = 1920
height = 1080
refresh = 120               # 0 = Gamescope default
filter = "fsr"              # linear | nearest | fsr | nis | pixel | sgsr
adaptive_sync = true
hdr = true

[[rules.known_issues]]
code = "compat-layer-required"     # unique slug within the rule
severity = "info"                  # info | warning | blocking
detail = "…"                       # one sentence, shown with the rule
```

### What a rule may not say

- `prefix_path`, `tool_id` and `working_dir` are not plan keys. They name one
  machine, so a shared rule that carried one would be a dead key everywhere
  else. The same applies to absolute paths in `args` or `env` values by
  convention; validation cannot prove intent, the review does.
- A shipped rule may not match `game_id`. Game ids are assigned by each Ludex
  server; only a local rule, written against this install, can know one.
- A display override must be self-contained: mode `gamescope` plus a valid
  width and height. A partial profile would be inert (the launcher refuses an
  incomplete Gamescope profile), which is exactly the dead key validation is
  meant to catch.
- A rule that changes nothing is invalid. If it only explains, it must carry at
  least one `known_issues` entry.

## Validation

Validation returns every problem at once and runs in three places:

1. **CI** — `cargo test` in `apps/desktop/src-tauri` parses and validates the
   shipped file (`compat_rules::tests::the_shipped_file_is_valid_and_current`).
   A malformed rule fails the desktop job before a release.
2. **At load** — `load_from` validates the cache and the local file. A layer
   that fails is skipped, logged once and the next layer is used. The shipped
   file is the fallback of last resort; if even that is corrupt (impossible in a
   passing build), the ruleset is empty and the app behaves as it did before
   this feature.
3. **After download** — `compat_rules_update` validates the signed file before
   replacing the cache. A file that fails keeps the previous copy.

Rules enforced (all tested):

- schema number equals `SCHEMA_VERSION`;
- ids are unique and match `[a-z0-9][a-z0-9-]{2,63}`;
- `match` has at least one condition;
- reason, source and confidence are present; reason and source are bounded;
- every plan key maps to `LaunchOptions` or `DisplayProfile`; TOML unknown
  fields are rejected by `deny_unknown_fields`;
- `env` goes through `linux_gaming::validate_env`, so malformed names, NUL
  bytes, oversized values and the reserved process variables (`PATH`, `HOME`,
  `LD_PRELOAD`, `LD_LIBRARY_PATH`, `PYTHONPATH`) are refused;
- `winetricks` goes through `linux_gaming::validate_verbs` and must already be
  normalised;
- prerequisites are from `runtime::plan::PREREQUISITE_IDS`;
- known-issue codes are unique within a rule, and each has a detail.

## Matching and precedence

At launch (#297) or when the "why" panel refreshes, the caller builds a
`MatchInput`: game id, optional exe SHA-256, optional engine tag, platform,
normalised arch, GPU vendor and executable kind. Every condition present on a
rule must hold. Nothing is guessed: an absent fact never satisfies a condition
(except `gpu_vendor = "unknown"`, which is an explicit no-probe condition).

Precedence, highest first:

1. **The user's launch options.** A rule fills defaults. If the stored mode is
   not `auto`, the env key exists, the winetricks list is not empty, the app id
   is set or a display profile exists, the rule reports `user-choice` instead of
   overwriting it. The panel shows "Your choice kept".
2. **Local rules.** `compat-rules.local.toml` is evaluated first and replaces a
   shipped rule with the same id. Two local rules that set the same field: the
   first wins, the second reports `rule-conflict`.
3. **The signed cache**, when its version is not older than the shipped file.
4. **The shipped file.**

Matching is pure and has no clock, network or filesystem access, so the rules
can be tested against fixture hosts.

## Independent update

The updater reuses the Tauri updater minisign key (`plugins.updater.pubkey` in
`tauri.conf.json`, decoded from base64). The public half is enough to verify;
the private key never ships.

- Endpoints: `data/compat-rules.toml` and `.minisig` on the `main` branch, so a
  rule change is live without a release. The raw host serves bytes; the
  signature is what makes it trustworthy.
- `compat_rules_update(force?)` fetches both, checks the signature against the
  configured key, parses and validates the file, refuses a version older than
  the cached copy, then writes the cache atomically. Any failure is returned in
  `message` and logged once; it is never a thrown error.
- The startup path spawns `compat_rules::spawn_startup_refresh` after the window
  and server are up. It is a detached task with a 24 h throttle. A launch never
  waits for it, and an offline machine simply keeps the shipped file.
- A build without a signing key ignores the cache entirely: unverified data
  must never drive a launch.

## Provider interface (#297)

The interface is intentionally narrow; it is the only surface #297 and #299
need.

```rust
// Load shipped + signed cache + local overrides. Never fails, never awaits.
let set: compat_rules::RuleSet = compat_rules::load_from(&app_data, public_key.as_deref());

// Facts the launch planner already has. `for_host` fills platform and arch.
let input = compat_rules::MatchInput { game_id, exe_sha256, engine, gpu_vendor, .. };

// Pure: base options in, complete options + extra args + reasons out.
let evaluation: compat_rules::RuleEvaluation = compat_rules::evaluate(&set, &input, &base);

// Same call, with the host facts filled from the running process (#297):
let evaluation = compat_rules::plan_for_launch(
    &set, game_id, &base, Some(&executable), engine, gpu_hint,
);
// evaluation.options      -> launch with these
// evaluation.extra_args   -> append after the user's startup command
// evaluation.prerequisites-> feed the runtime planner
// evaluation.rules        -> show the reasons ("why" panel)
```

`compat_rules::plan_for_launch` is the convenience wrapper used outside a Tauri
command. Both return the same data; neither writes anything.

Tauri commands (read-only except the fetch):

| Command | Arguments | Result |
| --- | --- | --- |
| `compat_rules_status` | — | `CompatRulesStatus`; active version, layer, counts, degradation notes, paths, update state |
| `compat_rules_report` | `{ gameId }` | `CompatRulesReport`; matched rules, reasons, changes, prerequisites, issues |
| `compat_rules_update` | `{ force? }` | `CompatRulesStatus`; fetches, verifies and caches; failures land in `message` |

## The "why" panel

`apps/web/src/components/game/compat-rules.tsx` renders the report read-only in
the Launch tab: the source badge ("rule 2026.11" or "learned locally"), the
confidence, the reason, the source, every field the rule set or kept, known
issues with their severity, and the prerequisites it asked for. Environment
variables are labelled by purpose ("Environment") and shown as `KEY = value` in
a monospace detail, never as the primary label.

## Adding a rule

1. Add the rule to `data/compat-rules.toml`. Prefer stable facts: PE header,
   engine, `exe_sha256`, host architecture. A game-specific claim needs a
   source you can cite.
2. Bump `version` (for example `2026.12`).
3. Run the desktop tests: `cargo test --lib compat_rules` from
   `apps/desktop/src-tauri`. The shipped-file test is the CI gate.
4. Sign the file with the release updater key (the Tauri CLI reads the
   base64 key file and writes a base64 `.sig`; the repository only ever carries
   the public half):

   ```sh
   pnpm --dir apps/desktop exec tauri signer sign \
     --private-key-path "$TAURI_SIGNING_PRIVATE_KEY_PATH" \
     --password "$TAURI_SIGNING_PRIVATE_KEY_PASSWORD" data/compat-rules.toml
   base64 -d data/compat-rules.toml.sig > data/compat-rules.toml.minisig
   rm data/compat-rules.toml.sig
   minisign -V -p ludex.pub -m data/compat-rules.toml -x data/compat-rules.toml.minisig
   ```

   The verifier accepts both the four-line minisign text and the base64 form
   Tauri writes, so either upload works; the repository uses the text form so
   `minisign -V` can audit it.

5. Commit both `data/compat-rules.toml` and its `.minisig`. The app reads the
   new rules on its next 24 h check, without a release.

A local experiment does not need a signature: create
`{appDataDir}/compat-rules.local.toml` with `[[rules]]` entries. Local rules may
match `game_id`, which makes "this game needed this on this machine" a one-line
file. Delete the file to return to the shipped database.

## Deliberately out of scope

- **#297** evaluates the provider at launch, detects the engine and GPU vendor,
  and applies prerequisites. This issue only provides `evaluate`.
- **#299** owns the settings tiers. The panel here is read-only; there is no
  editor for rules.
- No game-specific claim ships until it can be sourced. The initial database
  only states what the launch model and the executable header already prove.
