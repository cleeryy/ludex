# Desktop install pipeline

How a click on **Install** becomes an indexed game folder in the desktop app.
This document is the module map for `apps/desktop/src-tauri/src/install/` and
`src/net/`. It exists because the pipeline used to be one 8k-line module with an
828-line `execute`; the split (Wave 3 of #237) keeps the behaviour and gives
each step a name and a test.

## Module tree

```
src/net/                 transport-neutral primitives (no Tauri, no job row)
  mod.rs                 facade + docs
  retry.rs               RetryPolicy, RetryState, Retry-After, retry_at
  progress.rs            DownloadProgress, ProgressSink
  cancel.rs              CancelSet, Stopped

src/install/
  mod.rs                 public command facade (begin, list, pause, …)
  job.rs                 InstallJob/InstallParams/phases/queue moves/Failure
  plan.rs                pure queue + path planning, staging ownership
  scheduler.rs           begin/dispatch, worker lifecycle, finalizers, SQLite
  source.rs              local shortcut, ZIP stream, stream policy/retry
  download.rs            resumable transfer core + HTTP adapter
  files.rs               file-manifest transport (#231) + HTTP/SQLite adapters
  verify.rs              server digest, on-disk verification, space preflight
  commit.rs              commit stages (layout → transports → tree → index)
```

Dependency direction: `job` ← `plan` ← `scheduler` ← `source`/`download`/
`files`/`verify` ← `commit`. Nothing in `net` knows about a job; nothing in
`job`/`plan` performs I/O.

## Lifecycle

1. **Admission.** `scheduler::begin` validates the folders, writes the job row
   and calls `dispatch`. `dispatch` builds `pipeline::Candidate`s and asks the
   pure `pipeline::plan_next`; it claims the job under the registry lock and
   spawns `run`.
2. **Worker.** `run_job` sets up the heartbeat, then calls
   `commit::commit(app, entry, job_paths, download_slot, extract_slot)`.
3. **Commit stages** (`install/commit.rs`):
   1. `InstallLayout::prepare` resolves and creates the install root.
   2. `install_from_files` (policy `files`/`automatic`), then the local library
      shortcut, then `install_from_stream`, then `install_from_archive` run in
      that order and fill `PreparedSource`.
   3. `prepare_tree` copies a single-file game or extracts the archive, then
      resolves the executable (a stale configured path falls back to detection
      and is only a warning, #311).
   4. `move_into_place` renames staging to the final folder.
   5. `record_manifest` writes the #240 file manifest, `index_install` upserts
      the installed row, `remove_previous` deletes an update's old folder.
4. **Exit paths.** Done/paused/cancelled/failed/panicked all go through the
   finalizers in `scheduler.rs`, which release slots and the disk reservation,
   persist the row and re-dispatch.

## Invariants a change must keep

- **One extract/commit at a time.** `hold_extract` takes the single extractor;
  the download slot is released as soon as bytes are on disk (`#221`).
- **A stored checkpoint wins.** `plan::resolve_part_path` never relocates a
  `.part` after a setting change (`#226`); `plan::staging_is_protected` keeps a
  resumable job's staging folder out of the startup sweep (`#225`).
- **Cancel wins over pause.** `CancelSet::check` is biased cancel-then-pause;
  the commit stages use `check_cancelled` because a pause must not abort a tree
  that is already extracted.
- **No re-hash of the installed tree.** Extraction, the stream and the file
  manifest collect path/size/SHA-256 while writing (`#240`); the manifest is
  built from those entries.
- **The SQLite writes are the same shape.** `scheduler::persist` is the only
  writer of `install_jobs`/`install_job_params`; `commit` uses the same
  `upsert_installed`/`upsert_manifest` calls as before the split.
- **The command surface is unchanged.** `install/mod.rs` re-exports the exact
  names `commands.rs` used before (`begin`, `list`, `pause`, `resume_with_mode`,
  `reorder`, `check_space`, `space_ledger`, `QueueMove`, `ResumeMode`, …).

## Executable resolution after extraction (#311)

The archive is already on disk and verified when the commit stage looks for the
game binary. A configured executable (`install_game`'s `executable`, usually the
server/GameVault launch path) that is **not** in the extracted tree is therefore
a warning, never an error: the extraction is kept, staging is moved into place
and the job finishes `done`.

Resolution order for a relative or absolute configured path:

1. the path as written, resolved inside the extracted tree;
2. when it does not exist, the existing detector over the extracted files
   (`crate::bridge::pick_executable`): it prefers a program whose name matches
   the game title or the game folder that wraps it, prefers shallower and larger
   binaries, and skips installers, uninstallers, redistributables and crash
   handlers (`setup*`, `unins*`, `vcredist*`, `UnityCrashHandler*`, `Redist/`,
   `_CommonRedist/`, `Support/`, `Tools/`, …). The detected file becomes the
   recorded executable and the job message names it;
3. when detection finds nothing, the install still completes with no executable
   and the message says: *“The game installed but Ludex could not find an
   executable; pick one from the game's Launch options.”*

The only hard failure left in this step is a configured path that **exists**
but resolves outside the game folder: that is a broken manifest, not a stale
one. Digest mismatches, missing files and disk errors keep failing the job as
before; this rule applies only to executable resolution after a successful
extraction.

Manual fix: open the game page → **Launch options** → *Choose executable*
(`choose_executable`/`set_executable`, or `list_game_executables` for the ranked
list), which writes the pick back to the game. Launching with no executable
already shows the same "choose an executable" error instead of starting
anything.

## Retry, progress, cancel

- `RetryPolicy` is immutable (idle timeout, attempt cap, window, backoff,
  network gate, retry-now signal); `RetryState` counts attempts, tracks the
  retry window and trips the circuit breaker after three identical permanent
  errors. `delay` keeps the pre-split exponential ±25 % jitter; `delay_with_hint`
  adds the server's `Retry-After` as a floor capped at 30 minutes.
- `ProgressSink` is the one-method callback of a transfer. `download.rs`
  implements it as `JobProgressSink` (the only place that maps an update onto
  the `install://progress` payload); tests pass a closure.
- `CancelSet` owns the cancel/pause tokens and the biased waits: `select`,
  `sleep` (retry-now aware), `wait_online`, `check`, `check_cancelled`.

## Tests and the pipeline gate

`pipeline.rs` tests are the admission oracle and were not modified. The commit
tests pin the installed bytes: `the_installed_tree_digest_is_unchanged_by_the_pipeline_split`
extracts a representative archive (executable, empty file, 300 KB binary,
non-ASCII name) and compares a SHA-256 over the canonical
`path\0size\0sha256` list against the digest recorded before the split:

```
5751f1925d569a9df85fe9ef16b401e078ae9310e9618f1e69863ad0da2beaaf
```

`extracted_digests_match_the_manifest_the_second_pass_built` (#240) proves the
transport-collected manifest equals the old full walk, so the gate covers both
the install bytes and the manifest that is persisted.

## Adding a step

- Pure queue/path rule → `plan.rs` with a test next to it.
- New transport → `source.rs` for the policy, `download.rs` or `files.rs` for
  the transfer; report through `ProgressSink` and check `CancelSet`.
- New commit work → a named stage in `commit.rs`; update `PreparedSource` rather
  than adding mutable locals to `commit`.
- New public command → re-export it from `install/mod.rs`; do not widen the
  module's `pub` surface beyond what `commands.rs` needs.
