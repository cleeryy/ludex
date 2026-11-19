# HDR and adaptive sync (#292)

Two per-game switches decide whether Gamescope is asked for **adaptive sync**
(VRR) and **HDR output**. Both are opt-in, both apply only through Gamescope,
and both are disabled with a sentence when this machine cannot honour them.

Nothing in this feature writes to the desktop: no mode is changed, no connector
is rerouted and no compositor setting is saved. The desktop-untouched guarantee
comes from what is *not* passed to Gamescope (see below), not from a restore
step.

## What is probed

The probe reads Linux sysfs and the connector EDID once per compatibility scan
(`compat_scan`), and again before a launch that has one of the switches on. It
never opens the DRM device and never needs root.

One record is produced per **connected** output:

| Value | Source | Reading |
| --- | --- | --- |
| connection | `/sys/class/drm/card*-*/status` | `connected`; anything else is skipped |
| active output | `…/enabled` | `enabled` wins; with no enabled connector, a single connected output is the target |
| VRR | `…/vrr_capable` | `1` = supported, `0` = incapable, file absent = unknown |
| HDR | `…/edid` | CEA-861 extension with a **HDR Static Metadata** data block = supported; readable EDID without it = incapable; no/truncated EDID = unknown |
| max refresh | `…/edid` | highest vertical rate from the display range limits (`0xFD`) or a detailed timing descriptor, rounded to Hz |
| colour depth | `…/edid` | digital bit depth field of the video input definition (6, 8, 10, 12, 14 or 16 bits per channel) |

`unknown` means "the kernel or the EDID did not say", not "unsupported". An
unknown capability never blocks a switch: only an explicit `incapable` does,
because a driver that does not expose an attribute should not remove a feature.

The connector list, the active output and both verdicts are visible to the UI
through `compat_scan.displayCapabilities`; the game's **Display** section shows
the active output as one data line, for example:

```text
card0-DP-1 · VRR: supported · HDR: supported · 240 Hz · 10-bit
```

(`HDR: no metadata` and `VRR: incompatible` are the negative verdicts.)

## What Gamescope is asked to do

The switches only add flags to the nested Gamescope command (see
[GAMESCOPE.md](GAMESCOPE.md)):

```text
gamescope -w <width> -h <height> [-r <hz>] [--adaptive-sync] [--hdr-enabled] [--filter <name>] -- <game…>
```

- `--adaptive-sync` matches the output refresh rate to the game frame rate.
- `--hdr-enabled` asks for HDR output; the game still needs the Gamescope WSI
  layer to present HDR frames.

Both flags are emitted **only when the installed `gamescope --help` advertises
them**. Ludex never infers support from a version number. The wrapper stays
nested: Ludex never passes `-O/--prefer-output`, `-W/-H`, `-f/--fullscreen` or
`--backend drm`, so Gamescope cannot take over the output, change the desktop
mode or cover the other displays.

## Why a switch is disabled

The switches show the first applicable sentence, in this order:

| Situation | Sentence |
| --- | --- |
| no `gamescope` binary found | “No gamescope is installed, so adaptive sync has nothing to apply it.” / “…so HDR has nothing to apply it.” |
| the build does not list the flag | “The installed gamescope build has no `--adaptive-sync`.” / “…no `--hdr-enabled`.” |
| the active output is explicitly incompatible | “This output reports VRR as incompatible.” / “The output has no HDR metadata.” |
| capability unknown | switch stays available |

An already-enabled switch stays clickable so it can be turned off even after
the monitor or the Gamescope build changed.

## Fallback at launch

The UI is a snapshot; the monitor, the cable or the Gamescope build can change
before the game starts. When a switch is on but the capability disappeared, the
launch still runs and the request is dropped with a warning in the launch log
(`<appDataDir>/logs/launch-<game_id>-<session>.log`), for example:

```text
display: gamescope (hdr: false, adaptive sync: false, filter: fsr)
gamescope warning: The output has no HDR metadata; HDR was not requested.
gamescope warning: This output reports VRR as incompatible; adaptive sync was not requested.
```

The same log names the exact Gamescope command that was spawned, which is the
proof that the flags were applied (`--adaptive-sync`, `--hdr-enabled`).

## Check it by hand

List the connectors, their status, VRR capability and which one is enabled:

```sh
for c in /sys/class/drm/card*-*; do
  printf '%s status=%s enabled=%s vrr=%s\n' \
    "$(basename "$c")" \
    "$(cat "$c/status" 2>/dev/null)" \
    "$(cat "$c/enabled" 2>/dev/null)" \
    "$(cat "$c/vrr_capable" 2>/dev/null || echo missing)"
done
```

Inspect the EDID of the active output (the `edid-decode` package; `drm_info` or
`modetest -c` also work when installed):

```sh
edid-decode /sys/class/drm/card0-DP-1/edid | grep -A6 'HDR Static Metadata'
edid-decode /sys/class/drm/card0-DP-1/edid | grep -E 'Maximum Vertical|Detailed Timing'
drm_info | grep -i -E 'vrr|hdr'      # optional: kernel-side verdict
```

Check that the installed Gamescope advertises the flags:

```sh
gamescope --help 2>&1 | grep -E -- '--adaptive-sync|--hdr-enabled'
```

Then launch the game once and read the log (`<appDataDir>` is
`~/.local/share/cloud.cleeryy.ludex` for the installed Linux build):

```sh
LOG=$(ls -t ~/.local/share/cloud.cleeryy.ludex/logs/launch-*.log | head -1)
grep -E '^display:|^gamescope (command|warning):' "$LOG"
```

To prove the desktop was not touched, compare the outputs and modes before and
after the launch:

```sh
for c in /sys/class/drm/card*-*; do
  printf '%s %s %s\n' "$(basename "$c")" "$(cat "$c/status")" "$(cat "$c/enabled")"
done
```

The status/enabled pairs are identical before and after a launch with either
switch on.

## Tests

Fixture trees only; no test opens a real DRM device:

- `apps/desktop/src-tauri/src/display_caps.rs` — EDID fixtures with and without
  the HDR static metadata block, range-limit and detailed-timing refresh,
  10-bit colour depth, `vrr_capable` at `1`, `0` and missing, disconnected
  outputs skipped, active-output selection, truncated EDID, injected sysfs
  root, camelCase serialisation.
- `apps/desktop/src-tauri/src/linux_gaming.rs` — flag requested only when the
  build advertises it, output-incapable fallback warnings, unknown does not
  block, desktop launch stays byte-identical, no connector/exclusive flags.
- `apps/web/src/lib/display-profile.test.ts` — gating precedence (no Gamescope,
  unsupported build, incompatible output, unknown), summary and capability
  line.
