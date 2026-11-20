# Audio output routing (#289)

A game can play on a **chosen output device** while the rest of the desktop
stays on another. The routing is per game, applied through PipeWire or
PulseAudio with `pactl`; Ludex never bundles a sound server and never mutates
the desktop's own mixer defaults.

## What is read from the host

| Command | Used for |
| --- | --- |
| `pactl list short sinks` | sink index and full name |
| `pactl list sinks` | `Description:` for the UI (falls back to the name) |
| `pactl get-default-sink` | the sound server's current default |
| `pactl list sink-inputs` | stream index, current sink and `application.process.id` |
| `pactl get-sink-volume` / `get-sink-mute` | live volume state for the console OSD |
| `pactl move-sink-input <input> <sink>` | routing a game's stream |
| `pactl set-sink-volume <sink> +5%` / `-5%` / `set-sink-mute <sink> toggle` | volume up/down/mute |

The probe is injectable (`AudioRunner`), so the parsing, the fallbacks and the
exact argument vectors are unit-tested without PipeWire. A missing `pactl` is
reported as `pactl: false`, never as an error: the per-game panel explains that
audio routing needs PipeWire/PulseAudio, and a launch simply does not route.

## The test tone

`AudioService::test_tone` generates a short 440 Hz mono 16-bit WAV under the OS
temp directory and plays it on the selected sink with the player that is
actually installed:

- `pw-play --target=<sink> <file>` (preferred on PipeWire),
- `paplay --device=<sink> <file>` (PulseAudio).

Neither binary is invented: when both are absent the action fails with a
message naming both packages. The WAV is rewritten per process, not accumulated.

## Selection and fallback

The effective sink is resolved in this order:

1. the per-game sink (Launch → Audio output),
2. a `PULSE_SINK` entry in the game's launch environment, which is what the
   tier-2 "Audio device" switch writes (#299); this keeps both surfaces routing
   the same stream instead of overriding each other,
3. Ludex's **global override** (Settings in the console, or "Use this device for
   every game" on the game page),
4. the sound server's `get-default-sink`.

A stored sink that is not present in the current sink list is **never used**:
the launch falls back to the profile default and emits `audio://notice` with
`kind: "gameSinkMissing"` (or `defaultSinkMissing` for a stale global override).
The desktop toast and the console notice translate that event. When no sink at
all exists the notice is `noSink`; the game still starts.

The per-game value lives in `game_launch_options.audio_sink` (schema v26,
desktop SQLite only, never synced). The global override is
`audioDefaultSink` in `settings.json`.

## How routing runs

The route starts **after** the game process is spawned, on a worker thread:

1. read the process tree of the spawned root (`/proc` via `sysinfo`) — Proton,
   UMU and launchers open the audio stream from a descendant, not the root;
2. read `pactl list sink-inputs`, keep the streams whose
   `application.process.id` is in that tree;
3. move each matching stream whose `Sink:` is not the target;
4. repeat, up to 24 attempts every 250 ms (a bounded ~6 s window), while the
   stream appears.

The retry window covers the gap between the process starting and its audio
stream existing; a game that never opens one costs a few seconds of a thread,
not one for the whole session. Routing only ever names a sink that is present in
the same probe, so a resume cannot point at a device that disappeared.

If the device is unplugged **after** the move, PipeWire/PulseAudio re-routes the
stream to the new default by itself; Ludex does not mute, pause or kill the game.
Within the retry window the router re-resolves the plan and announces the
fallback instead of hammering the old name.

## Console OSD

In console mode the desktop OSD is out of reach (and may be disabled). Ludex
draws its own bar above the band:

- **Settings → Audio** changes the global device, volume (Left/Right, 5 %
  steps), mute and the test tone — all pad-reachable;
- keyboard/remote media keys (`AudioVolumeUp`, `AudioVolumeDown`,
  `AudioVolumeMute`, `XF86AudioRaiseVolume`, `XF86AudioLowerVolume`,
  `XF86AudioMute`) drive the same state while the console owns the key scope;
- every change reads the state back from `pactl`, so the bar shows what the
  sound server reports, then hides itself after 1.8 s.

## Missing binaries, degraded paths

| Situation | Behaviour |
| --- | --- |
| `pactl` missing | probe reports `pactl: false`; the panel shows the requirement; launch does nothing |
| `pw-play` and `paplay` missing | the Test action fails with a message naming both |
| stored sink unplugged | fallback to profile default + notice |
| global override unplugged | fallback to system default + notice |
| no sink at all | `noSink` notice, game keeps running |
| `pactl` command fails mid-route | logged at debug/warn, routing stops; the game is never touched |

## Tests

`apps/desktop/src-tauri/src/audio.rs` covers sink parsing, description
composition, sink-input parsing and PID-tree matching, selection and fallback,
the exact `pactl` argument vectors through an injected runner, the bounded
late-stream retry, volume/mute parsing, the WAV header and the missing-binaries
degradation. `process_tree.rs` covers the subtree helper (including a cyclic
snapshot). The web side tests the OSD state, clamp/expiry and the media-key map
in `lib/volume-osd.test.ts`.

## Manual check

```bash
pactl list short sinks
pactl get-default-sink
# launch a game with a per-game sink, then:
pactl list sink-inputs | grep -A2 'application.process.id'
pactl get-sink-volume <sink>
```
