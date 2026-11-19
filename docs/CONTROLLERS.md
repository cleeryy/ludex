# Controllers and launch order

A racing wheel can be enumerated as game controller 0. Anything that reads the
first controller — SDL, and therefore Wine/Proton's XInput — then binds to the
wheel, and the pad the player is holding never reaches the game. Nothing below
the game is broken: the kernel tags both devices, permissions are fine, and
`evtest` sees the pad. Only the order is wrong.

Ludex makes that order visible and gives each game a preferred controller
([#314](https://github.com/cleeryy/ludex/issues/314)). A true virtual pad that
rewrites the order for every program is out of scope and tracked separately
([#315](https://github.com/cleeryy/ludex/issues/315)).

## What Ludex reads

The diagnostics are read-only and never open a device:

| Source | Used for |
| --- | --- |
| `/proc/bus/input/devices` | name, `Vendor`/`Product`, `Handlers=` (`eventN`, `jsN`, `mouseN`, `kbd`) |
| `/dev/input/js*` | whether the joystick node still exists (plugged in) |

Every device is classified from its name and handlers:

| Role | Rule |
| --- | --- |
| Mouse | a `mouseN` handler, or "mouse" / "touchpad" / "trackpad" / "pointer" in the name |
| Keyboard | a `kbd` handler, or "keyboard" / "keypad" in the name |
| Wheel | "wheel", "driving force", G27/G29/G920/G923, Fanatec, T150/T300/T500, TMX, "racing" or "pedals" in the name |
| Gamepad | Xbox/X-Box, gamepad, pad, controller, DualSense, DualShock, Joy-Con, 8BitDo, GameSir, Steam Deck, "virtual" |
| Joystick | "joystick", flight stick, HOTAS, throttle |
| Unknown | anything else (power button, video bus…) |

A device is a **joystick** when it has a `jsN` handler, or when it looks like a
wheel/pad/joystick and has an `eventN` handler (`joydev` is not always loaded,
and SDL's udev backend reads evdev). `connected` means the kernel node still
exists: an unplugged device can stay listed in `/proc` for a moment.

`Settings → Profiles → Controllers` lists the joysticks with their role, name,
`vendor:product`, node (`js2`) and connection state, in enumeration order. The
same list, with a selection control, is in a game's **Launch** tab.

## Per-game preferred controller

The game page → **Launch** → **Controllers** chooser offers:

- **Automatic (host order)** — the default, no variable is added;
- any connected joystick with a `vendor:product` pair.

Choosing a pad does **not** hide every other pad. At launch, Ludex re-reads the
device list and hides only the *other connected* joysticks, then names the
chosen one for legacy consumers. A missing preference changes nothing.

The preference is stored in the device settings (`settings.json`,
`gameControllers`, keyed by game id) and never synced to the server: the pair is
a property of this machine's hardware. The overlay and Console mode keep using
every pad — only the launched game's environment changes.

## Environment written at launch

| Variable | When | Value |
| --- | --- | --- |
| `SDL_GAMECONTROLLER_IGNORE_DEVICES` | another connected joystick exists | comma-separated `0xVID/0xPID` pairs (`0x046d/0xc260`) |
| `SDL_GAMEPAD_IGNORE_DEVICES` | same | the same list, for SDL3 runtimes |
| `SDL_JOYSTICK_DEVICE` | the chosen device has a `jsN` node | `/dev/input/js2` |
| `SDL_GAMECONTROLLERCONFIG` | never | left to the player; see *Limits* |

### SDL semantics

- `SDL_GAMECONTROLLER_IGNORE_DEVICES` exists since SDL 2.0.5 and takes a
  comma-separated list of hexadecimal vendor/product pairs in the form
  `0xVID/0xPID`, with no spaces. SDL's game-controller enumeration (and Wine's
  XInput mapping, which uses it) skips a matching device, so the next controller
  becomes index 0.
- `0x0000/0x0000` is SDL's "ignore every game controller" wildcard. Ludex
  never writes it, and refuses to store it as a preference. `0xffff` is a
  per-field wildcard in SDL's parser, so a device whose vendor or product is
  `0000`/`ffff` is shown but can neither be chosen nor listed in an ignore
  list.
- `SDL_GAMECONTROLLER_IGNORE_DEVICES_EXCEPT` is deliberately **not** used: it
  hides everything that is not listed, so a pad plugged in after the choice
  would silently disappear.
- `SDL_GAMEPAD_IGNORE_DEVICES` is the SDL3 spelling of the same list. Ludex
  writes both names; each runtime reads the one it knows and ignores the other.
- `SDL_JOYSTICK_DEVICE` is the legacy SDL 1.2 / `js`-backend hint naming one
  device node. SDL2's udev backend and SDL3 ignore it; it is only there for
  older consumers that would otherwise open `js0` (the wheel).
- `SDL_GAMECONTROLLERCONFIG` is **not** generated. A mapping needs the SDL GUID
  plus a button/axis document, and guessing one could break a pad that already
  works. Xbox-compatible pads (including the common `045e:028e` clones) have
  built-in mappings in SDL and Proton. If a pad is enumerated as a plain
  joystick instead of a controller, add its mapping to the game's environment
  overrides (`SDL_GAMECONTROLLERCONFIG=…`) or use Steam Input.

### Wine and Proton

Ludex starts `umu-run`, the `proton` script or `wine` with the variables above
in the child environment; the game inherits them through the compatibility
layer. Wine's winebus/SDL backend and Proton's XInput path use SDL's game
controller list, so the ignored wheel no longer occupies index 0 and the pad
becomes the first controller the game sees. This is the same mechanism Steam
uses to hide a controller from a game.

It does not patch Wine, the prefix or the kernel: unplugging the wheel has the
same effect for one session, and nothing is written inside the prefix.

## Fallbacks and rules

| Situation | Result |
| --- | --- |
| No preference | environment untouched |
| Preferred device unplugged / node gone | environment untouched, warning in the launch log |
| `/proc/bus/input/devices` unreadable (non-Linux, sandbox) | environment untouched, warning |
| Only one joystick connected | no ignore list; `SDL_JOYSTICK_DEVICE` only |
| Two identical pads (same `vendor:product`) | both stay visible; SDL order decides |
| Another joystick without `vendor:product` | it stays visible; the summary says so |
| The player already set an `SDL_*` variable in the game's environment overrides | the per-game value wins |

A launch can therefore never hide every pad: the preferred device is excluded
from its own ignore list, and the list is only built from devices that are
connected *now*.

The launch log (`{appDataDir}/logs/launch-<game>-<session>.log` for a
compatibility launch) records the decision:

```
controller: preferred Microsoft X-Box 360 pad (045e:028e, js2); ignoring Logitech G29 Driving Force Racing Wheel (046d:c260, js0)
controller env: SDL_GAMECONTROLLER_IGNORE_DEVICES=0x046d/0xc260
controller env: SDL_GAMEPAD_IGNORE_DEVICES=0x046d/0xc260
controller env: SDL_JOYSTICK_DEVICE=/dev/input/js2
```

or, when the pad is absent:

```
controller warning: The preferred controller Microsoft X-Box 360 pad (045e:028e) is not connected; the launch environment is untouched and the game follows the host order (1 joystick(s) available).
```

Native Linux launches receive the same variables; a native launch without a
Gamescope wrapper writes no log file, so the warning goes to the app log.

## Limits

- The diagnostics read `/proc` and `/dev`; udev properties such as
  `ID_INPUT_JOYSTICK` are not queried. The classification is therefore
  name-based and can be wrong for an exotic device.
- Ordering shown is the `jsN`/`eventN` node order, which is what the kernel
  exposes. SDL's own enumeration can differ for identical models.
- A Flatpak/Snap game with its own device permissions sees the environment but
  may not see the host's device nodes; this does not change that.
- The preference is device-local and per game. Moving a game to another PC, or
  another user account, starts from **Automatic** again.
- A virtual pad that removes the wheel from the order for *every* program,
  including the overlay, is [#315](https://github.com/cleeryy/ludex/issues/315).
