# Window chrome (client-side decorations)

Ludex draws its own title bar on Windows and Linux; macOS keeps the native one.
The feature is #111.

## What the app ships

| Platform | Window frame | Controls |
|---|---|---|
| Windows 10/11 | `decorations: false` (base `tauri.conf.json`) | Ludex's top bar: wordmark, search, actions, then minimize / maximize-restore / close |
| Linux (GNOME, KDE, …) | `decorations: false` | same |
| macOS | native title bar + traffic lights (`tauri.macos.conf.json`) | none: the system draws them |
| Browser build | no window frame to control | none |

The base config creates the main window undecorated. `tauri.macos.conf.json`
re-declares the whole window with `decorations: true`: Tauri merge-patches
platform configs with RFC 7396, which replaces arrays instead of merging them,
so a partial `app.windows` entry there would drop `minWidth`, `resizable`, and
the rest.

## Setting: "Use the native title bar"

Settings → Appearance → *Use the native title bar* (off by default,
`nativeTitleBar` in the desktop `settings.json`).

- Applying it calls `set_native_title_bar`, which persists the field and calls
  `Window::set_decorations` on the live window: no restart.
- Startup reads the same field (`chrome::apply` in the Tauri `setup` hook), so
  the choice survives a relaunch.
- `save_settings` applies it too, so the setting is never out of sync when the
  rest of the desktop form is saved.
- macOS ignores the flag and always keeps its native chrome. The toggle is not
  rendered there.
- When the toggle is on, the web UI stops drawing the controls and the bar
  behaves like a normal page header. A `localStorage` mirror of the preference
  lets the first frame match the saved choice before the settings round trip
  completes.
- `tauri-plugin-window-state` also saves and restores `decorations` (it tracks
  all flags by default). The plugin's restore runs when the window is created,
  and `chrome::apply` in `setup` runs after it, so the stored setting always
  wins; both write the same value in practice.

## Behaviour

- **Dragging**: `data-tauri-drag-region="deep"` on the top bar. Tauri's injected
  script ignores clicks that start on buttons, inputs and links, so the search
  field, menus and window controls stay clickable. The action cluster and the
  controls opt out explicitly with `data-tauri-drag-region="false"`.
- **Double-click** on the bar toggles maximize. Tauri handles it natively for
  drag regions (`internal_toggle_maximize`, part of `core:window:default`); no
  custom handler is needed.
- **Maximize/restore icon** follows `isMaximized()`, re-read on `onResized`
  (debounced to 120 ms so a drag does not spam IPC).
- **Close** calls `Window::close()`, which raises `CloseRequested`. The existing
  handler still decides between hiding to the tray (`closeToTray`) and quitting:
  the custom button never bypasses the tray behaviour.
- **Fullscreen / Big Picture**: the bar is hidden while the window is
  fullscreen. Big Picture lives outside the shell and owns the fullscreen state
  already, so it never renders chrome.
- **Bare screens** (boot, login, setup, shared links) render the same controls as
  a 44 px cluster in the top-right corner, and the wordmark row is the drag
  strip, because these screens do not use the shell's top bar.
- **Hit targets** are 44 px wide and as tall as the bar (60 px desktop, 56 px
  below `1024 px`). Buttons are real `<button>` elements with `aria-label` and
  `title`, in minimize → maximize → close order, so Tab and screen readers reach
  them in that order. The focus ring is the global 2 px signal outline, pulled
  inset by 3 px so the window edge does not clip it.
- **Reduced motion** is covered by the global `prefers-reduced-motion` rule: the
  hover transitions are colour-only and collapse to 0.01 ms.

## Resize

Undecorated windows stay resizable on all eight edges:

- Windows: `RESIZABLE` keeps `WS_THICKFRAME` in the window styles; tao answers
  `WM_NCHITTEST` with `HTLEFT`/`HTTOPLEFT`/… when decorations are off and
  `resizable` is on. Tauri's runtime also attaches its `undecorated_resizing`
  handler when `set_decorations(false)` is called after creation.
- Linux: the same runtime handler is used.

No `startResizeDragging` handles were added: they would only be needed if the
frame stopped answering the hit-test, which it does not in Tauri 2.12.

## Known trade-offs

- **Windows 11 Snap Layouts**: the flyout that appears when hovering the native
  maximize button is unavailable. It needs a native `WM_NCHITTEST` answer of
  `HTMAXBUTTON`, and WebView2 receives the hit-test before Tauri does. Windows
  also only offers snap layouts for windows that can be ≤ 500 effective pixels
  wide, while Ludex has `minWidth: 1024`. Decision for #111: keep `minWidth` (the
  rail + content shell needs it) and accept the missing flyout — the toggle is
  the supported escape hatch. Implementing the flyout would need a Win32
  child-window overlay (for example `tauri-plugin-frame`) or a custom
  `HTMAXBUTTON` hit-test; both are out of scope here.
- **Window menu**: `Alt+Space` still reaches the system menu on Windows because
  the message loop is untouched, but there is no visible menu button.
- **Toggling decorations** rebuilds the frame, so the client area changes by the
  caption height once. The outer window size is preserved.
- **Windows 11 corners and shadow**: `shadow` is left at its default (`true`),
  which keeps the DWM shadow and the rounded corners that come from the standard
  overlapped window styles. This is expected from the window flags, not a manual
  test.

## Still to check by hand

Nothing in this list was verified on real hardware from the Linux dev box.

- Windows 10: undecorated frame, minimize/maximize/restore/close, double-click,
  all eight resize edges, tray close, DPI 100/125/150 %, high contrast.
- Windows 11: same, plus rounded corners, shadow, and the documented absence of
  Snap Layouts (then toggle the native title bar and confirm the flyout returns).
- Linux GNOME (Wayland and X11) and KDE: CSD, resize, maximize, focus ring.
- macOS: native traffic lights unchanged, no duplicate controls.
- Multi-monitor: dragging between displays with different scale factors.
- Touch/pen drag on the bar, and the bar under a screen reader (NVDA, Orca).
