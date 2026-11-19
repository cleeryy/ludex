# Gamescope display profiles (#287)

A game can run inside a **nested Gamescope** window at a resolution and refresh
rate chosen per game. The profile is optional: with no profile the launch
command is exactly the one Ludex built before this feature, and the desktop is
not touched.

## What Gamescope is here

[Gamescope](https://github.com/ValveSoftware/gamescope) is a micro-compositor.
In its default nested mode it is a normal window inside the running session: it
composites the game and scales it, but it does **not** own the display, change
the desktop mode or switch TTY. Ludex uses that mode only.

## Flags Ludex passes

Verified against `gamescope --help` and the `usage[]` table in
`ValveSoftware/gamescope`, `src/main.cpp` (2026-11-19). The command is:

```text
gamescope -w <width> -h <height> [-r <hz>] [--adaptive-sync] [--hdr-enabled] [--filter <name>] -- <program> <args…>
```

| Flag | Meaning in `gamescope --help` | Rule |
| --- | --- | --- |
| `-w, --nested-width <px>` | game width | required by a profile |
| `-h, --nested-height <px>` | game height | required by a profile |
| `-r, --nested-refresh <hz>` | game refresh rate | omitted when the profile says `0` (desktop rate) |
| `--filter <name>` | upscaler filter (`linear`, `nearest`, `fsr`, `nis`, `pixel`, `sgsr`) | only when the installed build lists that value in its help output |
| `--adaptive-sync` | enable adaptive sync/VRR output | only when the build advertises the flag and the output supports VRR (#292, see [HDR-VRR.md](HDR-VRR.md)) |
| `--hdr-enabled` | enable HDR output | only when the installed build advertises the flag and the output advertises HDR metadata (#292) |
| `--` | end of Gamescope options | always; the resolved game command follows |

The game command behind `--` is the one Ludex already resolved:

- native: the game binary, or `wine <game.exe>` when the mode is Native and the
  file is a Windows executable;
- UMU: `umu-run <game.exe>`;
- Proton through UMU (#279): `umu-run <game.exe>` with `PROTONPATH` set to the
  chosen Proton build;
- bare Proton (no UMU installed): `proton run <game.exe>`;
- Wine: `wine <game.exe>`.

The AppImage environment cleanup (#278) is applied **before** the wrapper
starts, because Gamescope is a host program: the bundle's `PYTHON*`, `LD_*` and
`GTK_*` variables and the mount entries in `PATH` never reach it.

### Flags deliberately not used

The issue sketch mentioned `-O/-W/-H/-f`; the installed help and the source show
that several of those mean something else:

- `-W/--output-width`, `-H/--output-height`: **output** size, used by the DRM
  backend. Nested mode sizes the game from `-w/-h`, so passing both pairs would
  duplicate the same intent with the wrong semantics.
- `-O/--prefer-output`: picks a connector in a DRM session. That is exclusive
  mode, out of scope here.
- `-f/--fullscreen`: makes the nested window fullscreen. A fullscreen window
  covers the desktop, which the acceptance criteria forbid.
- `--backend drm`: an exclusive compositor session. Never passed.

## Detection and capabilities

- Ludex looks for `gamescope` on `PATH` (and the usual `/usr/local/bin`,
  `/usr/bin`, `~/.local/bin` fallbacks), the same way it finds `umu-run` and
  `wine`.
- It probes once per scan with `gamescope --help`, which prints the version and
  the option table *before* opening any display, so it is safe headless.
- Support is read from that output, never from a version number:
  `--hdr-enabled` is offered only when the text contains it, and the filter list
  is the one printed after `--filter`. An older build simply shows fewer
  controls.
- `compat_scan` reports `gamescope: { path, version, hdr, filters }` or
  `null`; the launch options UI uses it to explain the state and hide
  unsupported controls.

## Fallback: no Gamescope, no failure

If a profile is enabled but `gamescope` is missing, unreadable or the binary
exits before the game:

1. the launch runs **un-wrapped** with the exact command it would have used
   without a profile;
2. `ludex.log` gets a warning naming the reason;
3. the game settings show that Gamescope is not installed and that the game will
   start on the desktop until it is.

The same fallback applies to an incomplete profile (width/height outside
320×200–7680×4320), an HDR request on a build without `--hdr-enabled`, and a
filter the build does not list. A bad profile never blocks a launch.

## Desktop restoration

Nothing is captured and nothing is restored, because nothing moves:

- Gamescope runs **nested**: a window inside the current session. It does not
  call the desktop's display API, does not change a mode, and does not switch
  the active output.
- The acceptance case from #287 ("1080p60 profile on a 240 Hz desktop never
  changes the desktop mode") therefore holds by construction, including on
  multi-monitor setups with different refresh rates: only the Gamescope window
  is affected, the other outputs keep their modes.
- Closing the game closes the window. A crash of the game, of Gamescope or of
  the wrapper leaves the desktop as it was; there is no half-applied mode to
  repair, even after a reboot.
- Exclusive mode (`--backend drm`) is **not used**. Ludex never asks for it,
  and this is asserted by the command-building tests (`-f` and `--backend` are
  absent from every generated command).

The nested window is sized by `-w/-h`; it is not resized when a monitor changes
resolution at runtime. The game keeps rendering at the profile resolution and
Gamescope scales it with the selected filter or its default linear filter.

## Failure path

`gamescope` exited before the game is visible in the existing launch watcher:

- the per-session log `<appDataDir>/logs/launch-<gameId>-<sessionId>.log` gets a
  header naming `display: gamescope`, the full `gamescope command:` line, the
  HDR/filter actually applied and every dropped request as
  `gamescope warning: …`;
- Gamescope's stdout and stderr are redirected to that same file, exactly like
  the UMU/Proton wrapper was before;
- if the process tree exits inside the immediate-exit window, the session is
  reported as a failed launch (with the exit code and the usual hint), not as a
  zero-minute play session.

## Profile precedence

The per-game profile is stored with the launch options (`display` in
`game_launch_options`). For one launch:

1. a per-game `gamescope` profile wins over the desktop default;
2. `desktop` (the default, and the value an older settings row deserialises to)
   runs the command unchanged — this is the no-op default;
3. there is no global Gamescope default today; a game without a profile always
   runs on the desktop.

## Limitations

- HDR is passed through only; whether it looks right depends on the Gamescope
  build, the compositor and the display. The switch exists only on builds that
  advertise `--hdr-enabled`.
- No output/connector selection: nested mode draws in the current session. Use
  the compositor's own window placement if the window lands on the wrong
  monitor.
- No frame limiter and no adaptive-sync flag yet; those are separate features.
- The profile is not synced to the server: like every launch option it is local
  to this device.
- A monitor resolution change does not pick a "nearest" mode: the window keeps
  the profile size, and Gamescope scales.
