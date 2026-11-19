# Power: keeping the machine awake while Ludex works

A download that dies because the screen blanked is indistinguishable from a
network failure, and a suspended machine stops the extraction, the shader
compile and the streaming host at the same moment. On a console or a headless
box nobody is there to move the mouse, so the idle timer always wins unless
Ludex says otherwise.

Ludex holds one `systemd-inhibit` lock for the lifetime of the process while it
has work to do, and releases it as soon as the work ends (#288).

## What triggers it

The inhibitor is held while **at least one** of these is true:

| Reason | Held from | Released when |
| --- | --- | --- |
| An install job is transferring, hashing, extracting or finalizing | the worker starts the job | the job is done, failed, paused or cancelled |
| An install is being checked from the game page (Verify) | the check starts | the check returns |
| A repair transfers files back into the install (Repair) | the repair starts | the repair returns |
| A game is running (including a ROM launched through an emulator) | the process is registered | the process tree has fully exited |
| A Moonlight stream started by Ludex is running | Moonlight is spawned | Moonlight exits |

Queued installs that have not started do not hold the lock: a queue waiting for
disk space must not keep a laptop awake forever. A paused job releases it too.

## What it blocks

```
systemd-inhibit --what=idle:sleep:handle-lid-switch --mode=block \
  --who=Ludex --why="Ludex is downloading Hades II" sleep infinity
```

- `idle` keeps the screen from blanking (DPMS off) and the session from going
  idle.
- `sleep` blocks suspend and hibernation.
- `handle-lid-switch` keeps a laptop working with the lid closed.

`shutdown` is deliberately **not** blocked: a user-requested power-off still
works, and so does `systemctl poweroff`.

On a machine without `systemd-inhibit` (or outside Linux) the feature is a
silent no-op. Status → the Settings → Host row and the banner simply report that
this machine keeps its own power settings; nothing errors.

## The banner

While the lock is held, the desktop shell shows a dismissible strip at the top
of the window:

> Keeping the machine awake while downloading (3 jobs)

The count is the number of install-ish reasons (installs, verifications,
repairs). With only a game or a stream running the sentence names that instead.
Dismissing hides it for the current set of jobs only; a new job shows it again.
The console shell has no banners, so it raises the same sentence once on its
notice surface when the work starts.

## The grace window

Settings → Downloads → **Keep the machine awake**:

> Keep the machine awake for N minutes after the last job

- Default **5** minutes, so a quick second job does not blink the screen between
  two installs.
- **0** releases the inhibitor the moment the last job ends.
- Bounded **0..60**: a longer window would silently disable the user's power
  settings for an hour.
- Saved in `settings.json` as `keepAwakeAfterMinutes`; a change applies to the
  running app immediately.

Only a new job cancels a pending window early; the window never extends itself.

## Checking it, and the no-stale-lock guarantee

```
systemd-inhibit --list
```

While work runs there is exactly one entry: `Who=Ludex`, `Why=Ludex is
downloading …`. When the last job ends (and the grace window expires) the entry
is gone.

Ludex guarantees that:

- **One child for the whole process.** Overlapping jobs share it; the set of
  reasons is refcounted by key, so a duplicated start or release cannot leak it.
- **Killed and reaped on release.** Both the `sleep infinity` child and the
  `systemd-inhibit` wrapper are killed, and the wrapper is waited on, so no
  zombie or orphan survives.
- **Released on exit.** The app exit hook stops the supervisor and drops the
  child before the process ends, including a quit from the tray.
- **Not restarted silently.** If `systemd-inhibit` exits on its own (a logind
  restart, say), Ludex logs it and does not spin retrying; the status row shows
  that no lock is held.

If an entry is ever left behind, the process is no longer running Ludex: remove
it with `kill <pid>` of the `systemd-inhibit` process shown by `--list`, and
open an issue with the app log (`{appDataDir}/logs/ludex.log`).
