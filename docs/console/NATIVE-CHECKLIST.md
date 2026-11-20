# Console native + streaming checklist (#270 §5)

The web evidence (`docs/console/evidence.md`) proves the input loop and the
layout in Chromium. This checklist proves the **shipped Windows build** under
**Sunshine/Moonlight**: the TV path the Console is designed for. It is manual
because it needs a real GPU, a real controller, a real stream and a real
borderless game.

**Status: not run.** No Windows host, no Sunshine host and no controller were
available to this lane. Every row is unchecked on purpose; a checked row
without an attached recording or log is not evidence.

## What you need

- Windows 11 host with the Ludex desktop build (WebView2), hardware video
  encode for Sunshine.
- One Xbox pad (xpad/XInput) and, ideally, one DualSense.
- Sunshine installed and paired with Moonlight on the TV.
- One game configured to launch **borderless windowed** (the overlay path) and
  one fullscreen-exclusive game (the overlay limitation path).
- A way to record: Moonlight's own statistics overlay or OBS on the host,
  60 fps, with the frame-time graph visible.

## Evidence rules

Every check needs one of these, attached to the issue or the PR:

| Evidence | How |
|---|---|
| Launch receipt | `%TEMP%\ludex-console-launch.json` — one JSON line per `--console` launch, last 16 kept. Proves a second instance reached an existing process. |
| Pad diagnostics | Open the Console with `?gamepadDebug=1` (`/#/console?gamepadDebug=1`); the panel shows pad id, mapping, profile, axes/buttons and the last action. Screenshot it. |
| Frame trace | Start Ludex with `set WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222`, attach `chrome://inspect` or Playwright `connectOverCDP("http://127.0.0.1:9222")`, record a Performance trace while holding the rail. Export the JSON. |
| Memory | Task Manager → Details, add the **Commit size** column, read the `ludex.exe` process after 30 s idle at 1080p and at 4K. Record MB. |
| Stream stats | Moonlight overlay (Ctrl+Alt+Shift+S) showing resolution/fps/bitrate/latency at the moment of the check. Screenshot. |
| Sunshine log | `C:\Program Files\Sunshine\config\sunshine.log` (or `%ProgramFiles%\Sunshine\sunshine.log`): the app launch and stream start/stop lines. Copy the relevant window. |

Do not record a pass from a windowed desktop session only: the point is the TV
path.

## Checklist

### A0. Controllers (per-game selection, #314/#315)

- [ ] **A0a — preferred pad.** Plug a wheel or a second pad next to the Xbox
      pad, set the Xbox pad as the game's preferred controller (Launch →
      Advanced), launch: the game sees the Xbox pad, the log header has a
      `controller env:` line, and the wheel does not take slot 0. Evidence:
      screenshot of the picker + the `launch-*.log` header.
- [ ] **A0b — absent preferred pad.** Unplug the preferred pad and launch: the
      environment is untouched and the log has a `controller warning:`. The game
      still gets a pad.
- [ ] **A0c — virtual gamepad.** On a Linux host with the udev rule installed,
      enable "Virtual gamepad" for one game: exactly one pad is visible to the
      game, Ludex still reads its own overlay input, and the bridge stops when
      the game exits (`/dev/input/js*` list returns to its previous state).
      Without `/dev/uinput` or permission the launch proceeds and Settings →
      Controllers says why.

### A. Process, window and second instance

- [ ] **A1 — cold console launch.** `Ludex.exe --console` from a terminal opens
      the Console fullscreen, no desktop chrome, no desktop toast/onboarding.
      Evidence: screenshot + `sunshine.log` launch line.
- [ ] **A2 — second instance promotes.** With Ludex already running on the
      desktop shell, run `Ludex.exe --console` again. The **existing** window
      promotes to the Console; no second window, no duplicate process. Evidence:
      `%TEMP%\ludex-console-launch.json` has a fresh line with the new PID's
      argv, and Task Manager shows one `ludex.exe`.
- [ ] **A3 — focus return.** From the Console, open Settings, back out with B,
      then Alt-Tab away and back. The ring is on the same target; no dead zone.
- [ ] **A4 — quit.** Power → Quit (the confirm defaults to Cancel: press B
      once first and verify nothing exits) → confirm. The process ends, the
      window disappears, Sunshine ends the session, no orphan `ludex.exe`.
      Evidence: Task Manager screenshot + `sunshine.log` stop line.
- [ ] **A5 — sleep.** Power → Sleep. The host suspends, wakes on input, the
      Console is still the active surface and the pad still drives it. If the
      row is disabled, read the reason it shows and record it (that is a pass
      for the capability probe, not for sleep).

### B. Controller

- [ ] **B1 — boot with a pad.** Reconnect the pad after the Console is up:
      `gamepadconnected` is picked up without a reload; the SystemBar shows the
      pad. Evidence: diagnostics screenshot before/after.
- [ ] **B2 — hot-unplug.** Unplug mid-navigation: the UI stops, the bar says no
      pad, and replugging resumes the same target. No reload, no stuck press.
      Evidence: short recording.
- [ ] **B3 — one press, one action.** Open the quick-actions sheet (Start), press
      D-pad Down once: the ring moves exactly one row (the #270 single-dispatch
      gate on WebView2). Then B returns to the same cover.
- [ ] **B4 — A/B swap.** Nintendo-layout pad: physical A confirms, physical B
      backs out, hint labels match the pad family.
- [ ] **B5 — haptics.** The pad rumbles on cue (Select/Confirm) at Standard and
      Strong, and stays silent at Off. No rumble for keyboard/mouse input.

### C. Sunshine / Moonlight at 1080p60

- [ ] **C1 — stream starts in the Console.** Moonlight launches the `Ludex`
      app; the Console appears fullscreen without a desktop flash or a resized
      window. Evidence: recording of app start.
- [ ] **C2 — two minutes of navigation.** D-pad walk through Home, Library,
      Game, Settings, Power and the quick-actions sheet. No stutter, no dropped
      input, no visible compression artefacts on focus rings or type.
      Evidence: Moonlight stats screenshot + recording.
- [ ] **C3 — rail scroll.** Hold D-pad right on the Stage rail for 10 s. The
      rail scrolls smoothly; frame time p95 ≤ 16.7 ms in the trace. Record the
      number even if it misses.
- [ ] **C4 — memory, FHD.** After 30 s idle on Home at 1080p, `ludex.exe`
      commit size < 300 MB. Record the MB.
- [ ] **C5 — no desktop leak.** No toasts, no desktop dialogs, no desktop
      sidebar while streaming. Errors appear as the Console notice card above
      the band.

### D. Sunshine / Moonlight at 4K

- [ ] **D1 — 4K stream.** Moonlight set to 3840×2160; same navigation pass.
      Type scaled by `--bp-u`, not clipped; overscan safe area empty.
- [ ] **D2 — rail scroll at 4K.** p95 frame time recorded (target 16.7 ms on
      hardware; a miss is a finding, not a pass).
- [ ] **D3 — memory, UHD.** After 30 s idle on Home at 4K, `ludex.exe` commit
      size < 380 MB. Record the MB.
- [ ] **D4 — overscan.** The 5 % crop of the TV is empty: no title, hint or
      focus ring lives in the outer band.

### E. In-game overlay over a borderless game

- [ ] **E1 — launch from the Console.** Play a borderless game from the
      Console. The launch plate ends, the game window takes focus, the Console
      stays behind it.
- [ ] **E2 — overlay opens over the borderless game.** Trigger the overlay
      (double-Home by default, or the configured chord). It appears above the
      game, not behind it; the game keeps rendering.
- [ ] **E3 — overlay is usable.** Tabs switch with the pad, the queue shows
      live progress, the library opens a game page.
- [ ] **E4 — close returns focus.** Closing the overlay hands focus back to the
      game (no black frame, no alt-tab). The game keeps running (the overlay
      never pauses it).
- [ ] **E5 — fullscreen-exclusive limitation.** Repeat E2 with a fullscreen
      exclusive game: the overlay must either appear or explain why it cannot
      (the Rust status carries the platform limitation). Record which one.

### F. Session edges

- [ ] **F1 — quit the game.** Quit the running game from the Console (Power →
      End game). Focus returns to the Console at the same target; fullscreen is
      restored; no black frame.
- [ ] **F2 — network loss.** Pull the network mid-download. The Console shows
      the offline notice, the library stays usable, no toast storm. Reconnect
      and confirm the queue resumes.
- [ ] **F3 — boot without a pad.** Start the stream with no controller: the
      Console is keyboard/mouse navigable, and the first pad press takes over.
- [ ] **F4 — restart.** Power → Restart. The app restarts into the same shell
      state (console mode preserved), no duplicate window.

## Sign-off

| Field | Value |
|---|---|
| Date | |
| Host build / commit | |
| Windows version | |
| WebView2 version | |
| Sunshine version | |
| Moonlight version | |
| Pad(s) | |
| 1080p p95 frame time | |
| 4K p95 frame time | |
| FHD commit size | |
| UHD commit size | |
| Result | ☐ pass ☐ pass with caveats ☐ fail |
| Deviations | |

Attach every evidence file listed in "Evidence rules" to the #270 thread. A
checked box without evidence is treated as not run.
