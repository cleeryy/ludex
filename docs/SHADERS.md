# Shader preparation (warm start) — #293

The first minutes of a DXVK title are spent compiling pipelines the driver has
never seen. The caches exist, but nothing fills them until the player has
already paid the stutter. Ludex can run the freshly installed game once, with
every shader cache pointed at a folder it owns, then stop it as soon as the
caches stop growing. The next launch reads those caches back.

This page states exactly what that does and does not do.

Related: [LINUX-GAMING.md](LINUX-GAMING.md) (launch modes, AppImage environment,
launch logs), [GAMESCOPE.md](GAMESCOPE.md) (display profiles), [POWER.md](POWER.md)
(the suspend inhibitor the run holds).

## There is no headless precompile

No portable, reliable "compile the shaders, show no window" switch exists:

- DXVK, VKD3D-Proton and the drivers compile **on demand**. Which pipelines a
  game needs is decided by the game while it runs.
- Most engines ship no precompile or benchmark mode, and the ones that do use a
  per-engine flag Ludex cannot know in advance.
- A software rasteriser would compile the wrong pipelines for the real GPU.

So the feature is a **warm start**, not a precompile: Ludex starts the game the
same way a normal launch does, with the cache paths pinned, and stops it after
the first frames. A window may appear while it runs. The UI says so, and the
panel is labelled "shader preparation" rather than claiming a headless pass.

## What the run does

1. The job waits in a background queue. One game at a time, never in the
   install queue's download or extraction slots.
2. The game is started through the same path as a real launch: same UMU, Proton
   or Wine plan, same Wine prefix, same working directory, and the per-game
   Gamescope profile when one is configured.
3. Every cache path is pinned to `{appDataDir}/shader-cache/{gameId}` (see the
   table below). The controlled run always writes there, so the size the UI
   shows is the size the next launch reads.
4. Ludex samples the cache folder once a second and stops the whole process
   tree when any of these happens:
   - the game exits on its own;
   - the cache stopped growing for 25 seconds (the usual case: the shader
     compilers went idle);
   - the time budget ran out (3 minutes by default, 30 s to 15 min);
   - the user cancels.
5. The result is written to the game's record: state, cache size, last prepared
   date, and the reason when it was stopped early.

The normal launch reuses the same variables whenever preparation is enabled
for the game or a prepared cache exists (a manual "Prepare now" therefore pays
off even with the automatic toggle off). That hand-off is what turns the warm
start into a quieter first session.

## Cache variables

| Variable | Folder | Filled by |
|---|---|---|
| `DXVK_STATE_CACHE`, `DXVK_STATE_CACHE_PATH` | `shader-cache/{gameId}/dxvk` | DXVK pipeline state caches |
| `VKD3D_SHADER_CACHE_PATH` | `shader-cache/{gameId}/vkd3d` | VKD3D-Proton D3D12 shader caches |
| `MESA_SHADER_CACHE_DIR`, `MESA_SHADER_CACHE_DISABLE=false` | `shader-cache/{gameId}/mesa` | RADV/ANV/GL driver caches (native and Proton games) |
| `__GL_SHADER_DISK_CACHE`, `__GL_SHADER_DISK_CACHE_PATH`, `__GL_SHADER_DISK_CACHE_SKIP_CLEANUP` | `shader-cache/{gameId}/nvidia` | The proprietary NVIDIA GL cache |

The folder lives in the app data directory, not in the Wine prefix: a prefix
reset or a game uninstall must not silently delete a cache the user asked to
keep, and a prefix on a slow drive would make every launch read it.

A per-game environment override wins at launch. If the game sets
`DXVK_STATE_CACHE_PATH` itself, the prepared cache is not read back — the
override is a deliberate choice and is never replaced.

## What the user sees

- **Game page → Launch → Shader preparation**: "Prepare after install" toggle,
  "Prepare now", "Clear cache", cache size, last prepared date, time budget and
  the last message (failure, cancellation, or how the run was stopped).
- **Downloads**: a "Shader preparation" section listing queued, running and
  recent runs, with cancel and clear. The section only appears when a game has
  a record.
- **Settings → Device**: the global "Prepare after install" default and the
  time budget.

The size is one recursive walk of the cache folder; symlinks are skipped so a
cache linked elsewhere is not counted twice.

## Failure behaviour

Preparation never touches install state:

- a failed spawn, a crash, a non-zero exit, a clean exit that cached nothing, a
  timeout with no cache written, or a cancellation all leave the game
  **launchable exactly as before**;
- the reason is stored on the record and shown in the panel and in Downloads;
- whatever was written before the failure is kept, so a partial warm start still
  helps the next launch;
- "Clear cache" deletes the folder and resets the record. It is refused while a
  run owns the folder.

## Lifecycle details

- The queue is in memory. A restart during a run turns the stale `queued` or
  `running` record into an honest cancellation ("Ludex closed before the shader
  preparation finished").
- The run holds the suspend inhibitor (#288) with the `prepare` reason, so a
  console does not blank or suspend mid-warm start. The banner names the game.
- "Prepare after install" is queued when the install job reports `done`. It is
  never part of the install: the job is already finished, and the first launch
  is not blocked or delayed.
- A game that is currently running is not prepared; the record says so.

## Reproducing it by hand

The environment is the whole trick:

```sh
CACHE="$HOME/.local/share/dev.cleeryy.ludex/shader-cache/42"
mkdir -p "$CACHE"/{dxvk,vkd3d,mesa,nvidia}
DXVK_STATE_CACHE=1 DXVK_STATE_CACHE_PATH="$CACHE/dxvk" \
VKD3D_SHADER_CACHE_PATH="$CACHE/vkd3d" \
MESA_SHADER_CACHE_DIR="$CACHE/mesa" \
umu-run /games/Hades/Hades.exe
# wait until the cache stops growing, then quit the game
du -sh "$CACHE"
```

The `{appDataDir}` path differs per platform; the panel prints the exact folder
under the controls.

## Tests

`apps/desktop/src-tauri/src/shader_prep.rs` covers the rules without launching a
game: cache path resolution, the environment catalog, size accounting
(nested files, symlinks, a missing folder), the plateau and time-budget stops,
cancellation, a non-zero exit (failure that keeps the cache), an injected runner
that receives the cache environment, and the record mapping. The queue is tested
for deduplication, cancellation of a queued game, and the per-game/global
precedence of "prepare after install".
