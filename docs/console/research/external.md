# External research: Console / TV / 10-foot UI patterns for Ludex Console 2.0

**Prepared:** 2026-11-16
**Scope:** read-only external research. No repository files were touched. All URL fetches performed 2026-11-16.
**Audience:** whoever designs and implements Ludex Console 2.0 (Tauri 2 desktop shell, shared React web app, existing `/bigpicture` route and Console screen).

## 0. How to read this document

Three kinds of statements appear below, and they are labelled differently on purpose:

- **Documented** — stated in a vendor spec, developer doc, or standards document. Cited with URL and, where the page exposes one, a last-modified/publication date.
- **Reported** — behaviour I could not exercise first-hand (I cannot drive a PS5, a Steam Deck, or a Roku here). Taken from vendor marketing, press coverage, or user reports; cited, and treated as weaker evidence than documented behaviour.
- **Opinion** — my judgement, marked as such. Every "what to copy / what to avoid" verdict is opinion unless it restates a documented constraint.

A note on the hardest constraint in this whole document: **Ludex Console is not a smart-TV app.** It runs in a Tauri WebView (WebView2 on Windows, WebKitGTK on Linux). That relaxes the brutal memory ceilings that Tizen and webOS impose, but it does *not* relax the 10-foot design constraints, and it introduces a risk that Chrome-based TV work hides: **WebKitGTK is the weak runtime** for gamepad input, compositing, and video. Several recommendations below exist specifically because of that.

---

## 1. What changed in 2024–2026 (read this before copying anything)

Products in this space moved in 2025–2026, and some of the "console UI canon" you may remember is stale:

| Change | Date | Why it matters for Console 2.0 | Source |
|---|---|---|---|
| **Valve announced Steam Machine, new Steam Controller, Steam Frame**; shipping 2026, delayed by memory/storage shortages | Announced Nov 2025; 2026 ship | SteamOS becomes a *living-room console OS*, not a handheld OS. Steam's home screen is now the reference for a 2026 TV console. Deck Verified expands to Machine/Frame. | [Engadget, 2026](https://www.engadget.com/gaming/pc/valves-steam-machine-launches-in-2026-everything-we-know-so-far-200458597.html); [Steamworks announcement, Nov 2025](https://steamcommunity.com/groups/steamworks/announcements/detail/563639634283728255) |
| **tvOS 26 "Liquid Glass"** redesign | Announced 2025-06-09, shipped Sept 2025 | Real-time refraction/blur is **hardware-gated**: only 2nd/3rd-gen Apple TV 4K get it. Apple itself ships a design that degrades on older boxes. | [Apple Newsroom, 2025-06-09](https://www.apple.com/newsroom/2025/06/apple-tv-brings-a-beautiful-redesign-and-enhanced-home-entertainment-experience/) |
| **Fire TV moves to Vega OS** (Linux, React Native + Chromium WebView) | Announced Sept 2025; new sticks since fall 2025 | The largest TV platform dropped Android for a **web-tech stack**. Web TV app knowledge now transfers directly to Fire TV. | [Amazon, Sept 2025](https://developer.amazon.com/apps-and-games/blogs/2025/09/announcing-vega-os); [Vega Web Apps, 2025-09-30](https://community.amazondeveloper.com/t/getting-started-with-vega-web-apps-and-webview/20262) |
| **PS5 "Welcome hub"** replaces the Explore tab with customizable, resizable widgets and presets | 2024-09-12, phased | The console home screen is becoming a **configurable dashboard**, not a fixed row. | [PlayStation Blog, 2024-09-12](https://blog.playstation.com/2024/09/12/ps5-system-update-adds-welcome-hub-party-share-personalized-3d-audio-profiles-adaptive-controller-charging-and-more/) |
| **Plex "new Plex Experience"** unified redesign across clients, incl. TV/Roku preview | Preview fall 2024, rollout 2025 | A cautionary tale: reported performance regressions and usability complaints on TV hardware and <4 GB devices. | [Plex blog](https://www.plex.tv/blog/its-go-time-the-new-plex-experience-is-here/); [Plex forum thread](https://forums.plex.tv/t/new-ui-is-an-awful-experience/931048?page=23) |
| **GameVault migrates to a web/Tauri client** (`gamevault-frontend`, React 19 + Vite + Tailwind 4 + Tauri + React Router) | active, last push 2026-11-09 | The closest analog to Ludex has converged on *the same architecture you already have* — and on two update channels (stable/unstable). | [GitHub: Phalcode/gamevault-frontend](https://github.com/Phalcode/gamevault-frontend) |
| **CSS Spatial Navigation still unimplemented**; editor's draft moved to `css-spatial-nav-1` with at-risk features | editor's draft 2026-05-14; WICG page updated 2025-09-09 | There is no browser feature to lean on. You will write or vendor the algorithm. | [drafts.csswg.org/css-spatial-nav-1](https://drafts.csswg.org/css-spatial-nav-1/); [WICG spatial-navigation](https://wicg.github.io/spatial-navigation/) |
| **Jellyfin 10.11** stable, Android TV 0.20 in flight | 2025–2026 | OSS media clients iterate slowly on TV shells; nav inconsistencies persist across clients. | [Jellyfin blog index](https://jellyfin.org/posts/) |
| **Switch 2 UI is nearly identical to Switch 1**; menus render at 1080p | June 2025 | Even Nintendo shipped *no* redesign. Simplicity won. | [GameSpot](https://www.gamespot.com/articles/switch-2s-main-menu-isnt-that-different-from-the-one-on-nintendo-switch/1100-6530604/); [ResetEra: menus at 1080p](https://www.resetera.com/threads/switch-2-renders-its-menus-at-1080p.1208412/) |

---

## 2. Comparison matrix

Columns are collapsed deliberately. Everything here is **documented** unless marked *(reported)*; "copy/avoid" is **opinion**.

### 2a. Consoles and platform shells

| Product | Navigation model & home hierarchy | Focus & selection | Input | Lifecycle (boot / resume / power) | Content ops (downloads / store / social) | Motion & sound | Accessibility | Copy / avoid |
|---|---|---|---|---|---|---|---|---|
| **Steam Big Picture / SteamOS** (Deck UI; Machine 2026) | Top-level rail (Store, Library, Community, Downloads, Settings) + a library-first Home of shelves ("Recent", "Great on Deck"). Left rail is the spine; rows are the body. | Focus = scale-up + outline + background scrim; **selection is a separate press** (A). Focus never auto-moves on load. | Full parity: gamepad, trackpads, gyro, keyboard/mouse; Steam Input remapping per game; on-screen keyboard. | Boots to Home; per-game **suspend/resume**; power button suspends. *(reported)* | Dedicated Downloads page with queue/reorder/throttle; full Store + wishlist; friends/chat/notifications unified in one overlay. | Fast, short transitions; UI clicks; no cinematic wipes. | UI scale + magnifier in SteamOS settings *(reported; verify on device)*. | **Copy:** library-first home, one overlay for social/notifications, real download queue UI, suspend-resume per game. **Avoid:** assuming you have a store — Ludex does not. Steam's "Featured/New" shelves make sense because Valve sells things; for a personal library they are empty-shelf theatre. |
| **Xbox (dashboard + Cloud Gaming web)** | Vertical tabs (Home, Store, Game Pass, Media, Apps) with horizontal shelves; heavy promotional tiles. `xbox.com/play` is a **grid of tiles** with a filter row. | White focus ring + slight scale; A selects; B backs; **View/Menu buttons** for context. | Controller-first. Cloud Gaming requires a Bluetooth/USB controller on most devices; **some games support mouse+keyboard**, which Microsoft documents explicitly. | Instant-on vs Energy saver; **Quick Resume** keeps multiple games suspended across power cycles; Remote Play wakes the console. | Queue with "suspend installation"; store + subscriptions; parties/clubs/friends. | Moderate; transitions are quick; sound on focus is minimal. | Strong: high contrast, narrator, copilot, remappable controller (Xbox Accessibility Guidelines). | **Copy:** Quick Resume (multiple resumable states), controller-first web layout, explicit "mouse+keyboard supported on some titles" honesty. **Avoid:** ad/tile density. A personal library has nothing to advertise; those tiles become noise and extra D-pad presses. |
| **PlayStation 5** | One horizontal row of games + **Welcome hub** (customizable widgets: storage, battery, friends, trophies; resizable, preset layouts) + **Control Center** overlay from the PS button. Settings are an overlay, not a separate app. | White border + scale + background art swap; X selects; O/Circle backs. | DualSense; M+KB on supported titles; no pointer. | Rest Mode resume; game Switcher; downloads can run in rest. | Downloads/queue in Control Center; PS Store; parties/activities. | Very fast; light sound design; no long wipes. | System-level zoom, screen reader, colour filters, button remap. | **Copy:** Control Center as a *cheap overlay* instead of a full-screen settings app; Welcome hub only if widgets carry **real state**. **Avoid:** widgets that display system trivia (storage %, battery) — Ludex-specific equivalents are downloads, updates, saves, server health. Also avoid Sony's settings nesting. |
| **Nintendo Switch 2** | **One row** of tiles: all games + fixed buttons (News, eShop, Album, Controllers, Settings, Sleep). No shelves, no recommendations. | Border + slight scale; A select; **+** opens options; **−** nothing. | Joy-Con/Pro Controller; touch; no pointer on TV. | Instant resume from sleep; profile chosen at boot/first launch. | eShop; no background download queue surfaced in UI (reported). | Minimal; fast; tiny sounds. | Limited but consistent; system-level zoom. | **Copy:** the radical part — *everything is at most two presses away*, and the home screen never reflows. **Avoid:** no sort/search/filter for large libraries. This is the single biggest flaw to not copy: at 300+ games a single row is unusable, and Ludex libraries will be large. |
| **Apple TV / tvOS 26** | App-defined top tab bar (Watch Now / Apple TV+ / Store / Search) + horizontal shelves; the OS provides **Top Shelf** above the app row. | OS-level **focus system**: directional, **every on-screen element must be reachable**; parallax "lift" on focus; selection is a separate click. | Siri Remote (touch surface + clickpad), game controllers, keyboard, iPhone-as-remote. | Instant wake; apps handle their own profiles ("Who's watching"); Apple pushes *easy, infrequent sign-in* and **automatic profile switching**. | Store rentals/purchases; no download queue in the TV UI. | Liquid Glass: real-time refraction, **hardware-gated** to 2nd/3rd-gen 4K. | Excellent system-level: VoiceOver, reduce motion, reduce transparency, high contrast. | **Copy:** Apple's focus rules verbatim — *never move focus without user interaction*, and the documented exception when the focused item disappears. **Avoid:** real-time blur as a baseline. Apple ships it only where the GPU can afford it; on a mini-PC you cannot assume that. |
| **Android TV / Google TV** | "For you" shelves + apps row; D-pad traversal, long-press for context menus. | Elevation + scale + ring; **instant, distinct feedback on every keypress** (explicit requirement). | D-pad remote, gamepad, keyboard; voice. | Fast boot to launcher; apps restore their own state (no OS-level quick resume). | Play Store; no unified queue. | Moderate; Google's own guidance warns against long/large motion. | TalkBack, switch access; TV-specific a11y guidance. | **Copy:** the concrete layout math (§3.5) and "one clear purpose per direction". **Avoid:** recommendation soup and autoplay trailers — hostile on a library you already own. |
| **Tizen Smart Hub / LG webOS launcher** | Bottom app bar + "Recent" strip; webOS adds a pointer-driven Magic Remote layer. | Ring/scale; Back is a first-class key (Tizen `10009`). | Remote, Magic Remote pointer, gamepad (webOS gates HID via `cloudgame_active`). | Apps are killed aggressively; **relaunch semantics are platform-managed** unless you opt in (`handlesRelaunch`). | Store only; no queue. | Simple; cheap. | Audio guidance via ARIA opt-in (`supportsAudioGuidance`). | **Copy:** explicit back-stack contracts; the platform's own "who owns history" switch. **Avoid:** depending on pointer or hover. |
| **GeForce NOW** (app + web) | "Continue playing" shelf, then library grid; store/search tabs. | Ring + scale; A launches; overlay via a chord. | Controller, M+KB; gamepad passthrough to the streamed game. | Session-based: **rig queue with position/ETA**, then stream. | No downloads; library sync from stores; queue is the "download" equivalent. | Minimal; loading states dominate. | Basic. | **Copy:** queue as a *first-class state* with position, ETA, and cancel — the right model for any long operation (installs, scans, downloads). **Avoid:** modal queue that blocks the whole UI with no escape. |

### 2b. Web/TV apps and open-source launchers

| Product | Navigation model & home hierarchy | Focus & selection | Input | Lifecycle | Content ops | Motion & sound | Accessibility | Copy / avoid |
|---|---|---|---|---|---|---|---|---|
| **Plex** ("new Plex Experience", 2025) | Library-centric: Continue Watching / Recommended / Libraries / Watchlist; unified nav across clients; denser artwork. | Highlight + scale; selection separate. | Remote, gamepad, keyboard, pointer (varies by client). | Profile pick at launch; server connection state is visible. | Server-side transcoding, downloads on mobile only; watchlist. | Moderate. | Improving but uneven across clients. | **Copy:** "Continue" as the first row, always; profile gate at launch. **Avoid:** shipping a cross-client redesign simultaneously — reported TV regressions and low-RAM complaints are exactly the failure mode to dodge. |
| **Jellyfin** (Android TV 0.20 / web) | Left nav rail + horizontal rows on TV; sidebar + rows on web. **Nav model differs per client.** | Highlight + scale; inconsistent visuals between clients. | Remote, gamepad, keyboard. | Reconnect flow on server loss; no unified resume UX. | Server tasks (scan, transcode) not surfaced as a queue in the TV client. | Minimal. | Basic; relies on Compose/ARIA defaults. | **Copy:** library-first with zero commercial surface. **Avoid:** letting each client invent its own focus visuals and back semantics. This is the OSS failure mode Ludex must not repeat: *one* Console, *one* focus language. |
| **Netflix (TV)** | Rows: Continue Watching first, then curated rows; profiles at launch; "Top 10"; My List. | Focus triggers **autoplay preview** after a dwell. | Remote, gamepad, keyboard; OSK for search. | Profile gate; deep-link resume. | No downloads on TV; My List. | Autoplay previews = motion + audio by default. | Caption/subtitle styling; audio description; **reduced-motion hostility** from autoplay. | **Copy:** profile gate (shared TV = shared account), resume-first row ordering. **Avoid:** focus-triggered autoplay. On a personal library it is pure cost: GPU/CPU burn, audio surprise, motion-sensitivity problems, and it competes with the user's own intent. |
| **YouTube TV / Disney+ (TV apps)** | Dense grids + brand hero rails. | Ring/scale; hero often grabs focus on load. | Remote/gamepad; voice search. | Deep links into playback; profile per service. | No local queue. | Heavy hero animation on load. | Subtitle/audio options; inconsistent focus contrast. | **Copy:** deep-link-to-resume. **Avoid:** hero banners that steal focus on mount, and grids that change column count mid-scroll. |
| **Kodi (Estuary)** | Vertical section menu (Movies, TV, Music, Add-ons…) with **per-section widget rows** ("Recently added", "In progress"). | Coloured highlight + scroll; `Enter` selects; `Backspace/Esc` backs. | Remote, keyboard, gamepad via keymap; fully remappable. | Resumes last section; playback OSD overlay. | Library scan as an explicit job; add-on manager. | Minimal, configurable. | Skin-dependent; zoom + TTS add-ons. | **Copy:** per-section widgets driven by *your own* data (recently played, in progress, installed-but-never-played). **Avoid:** settings sprawl and skin-specific focus rules that authors can break. |
| **EmulationStation DE** | Carousel of systems → vertical gamelist → metadata/detail. | Carousel focus + list highlight; A select, B back, **held directions repeat**. | Gamepad-first; keyboard; remap UI in-app. | Per-emulator launch; returns to list. | Scraper jobs; no network store. | Simple transitions; optional sounds. | Weak (small text, low contrast). | **Copy:** the carousel→list→detail Z-axis, and held-direction repeat tuning. **Avoid:** carousel as the *only* way to browse a big library. |
| **Playnite (Fullscreen)** | Cover grid + filter/sort sidebar; "Fullscreen" is a mode of a desktop app. | Ring/scale; controller and keyboard. | Controller, keyboard, mouse. | Mode switch from desktop; no suspend. | Install/update queue delegated to store integrations. | Theme-dependent. | Theme-dependent; contrast often poor. | **Copy:** filter + first-letter jump for huge libraries. **Avoid:** theming that can break focus visibility — themes must not be able to remove the focus ring. |
| **Heroic / Lutris** | Desktop-first: list/grid + sidebar; mouse-centric. | Desktop focus semantics (tab order, hover). | M+KB primary; controller for games, not for the shell. | Launches games via Proton/Wine; no shell-level resume. | Download/install progress per game; no unified queue. | Desktop motion. | Desktop a11y only. | **Copy:** nothing structural. **Avoid:** bolting a "console mode" onto desktop focus semantics — it inherits hover assumptions and tab order. This is the trap Console 2.0 must avoid architecturally. |
| **GameVault** (web/Tauri client) | Library grid + download/install flow; admin screens; server-configurable. | Browser-default focus (a weakness). | M+KB; controller not first-class. | Tauri updater with **stable/unstable channels**, signature verification, feed fallback. | Backend download jobs surfaced in client. | Minimal. | Basic. | **Copy:** the updater model (two channels, signed artifacts, graceful fallback) — it maps 1:1 onto Ludex's Tauri updater. **Avoid:** shipping a web UI with browser-default focus on a 10-foot surface. |
| **Moonlight / Sunshine** | Host list → app list → stream; explicit pairing state. | Ring/scale; A connect, B back. | Full gamepad passthrough; M+KB capture; overlay chord. | Session lifecycle: pair → connect → stream → disconnect; clear error states (host offline, busy, wrong PIN). | No downloads; "app already running" state. | None. | Minimal. | **Copy:** the two-step host→app model with honest error states (offline/busy/PIN). **Avoid:** ambiguous "connecting" with no cancel. |

---

## 3. Cross-cutting principles for a good console UI

### 3.1 Hierarchy: two levels, no more

Every product above that feels good has exactly **two structural levels**: a spine (rail/tabs/single row) and a body (shelves/grid). PS5 collapses to one level and uses an overlay for everything else. Switch collapses to one level and accepts the cost.

Three or more levels (tab → shelf → sub-shelf → detail → sub-detail) is where D-pad UIs die, because each level adds a back-press and users lose the mental map. Android's own TV guidance makes the same point from the other direction: design each direction to have **a clear purpose**, and don't send focus back and forth between panels — it creates "unnatural focus paths".

**Opinion:** Console 2.0 should be spine + body + **overlays** (detail, settings, downloads), never a fourth navigable level.

### 3.2 Density: the number that decides everything

Concrete, documented numbers to anchor on:

- Android TV: design at **960×540 mdpi** (1 px = 1 dp), target 1080p assets.
- 12-column grid, **52 dp columns**, **20 dp gutters**, 58 dp side margins, 4 dp vertical spacing.
- Card widths by column count: 1 → 844 dp, 2 → 412 dp, 3 → 268 dp, 4 → 196 dp, 5 → 124 dp.
- Overscan safety: **5 %** — 48 dp left/right, 27 dp top/bottom (table says round to 24 dp; note says **most modern TVs no longer overscan**, but cheap STBs, projectors, and "cinema/zoom" picture modes still crop).

What that means in practice: at 1080p, **4 columns of ~196 px cards** is the comfortable ceiling for a library grid; 5+ columns works only for text-light posters and only if focus scaling has room (Android explicitly warns to leave padding for the focus state so cards don't overlap).

### 3.3 Type at distance

- **Apple tvOS minimum text sizes: 29 / 23 / 17 / 12 pt** across its text styles (HIG Typography). If you take one number: **body text ≥ 24 px at 1080p**, and never below ~18 px for anything the user must read.
- Android TV: prioritize large type; avoid decorative faces; use sans-serif for body and labels; thin weights are illegible at distance.
- Practical rule for 1080p logical design: UI labels 18–20 px, body 24 px, row titles 28–32 px, screen titles 40–48 px. Then scale the whole tree with one root variable, not per-component media queries.

### 3.4 Focus predictability (the thing users actually feel)

This is where consoles differ from web apps, and where the vendor docs are unusually explicit:

- **Never change focus without user interaction.** Apple states it directly, with a single documented exception: when the focused item *disappears* and the user is moving with a discrete directional input, move focus to a nearby remaining item so the indicator stays findable. Otherwise, hide the indicator.
- **Every element must be reachable** on a directional-focus platform (tvOS), unlike desktop keyboard modes where buttons may be skipped.
- **Focus must be visible at a glance** and consistent across screens: one ring/scale language, one accent, everywhere.
- **Do not re-anchor focus on data load.** When a row's contents arrive asynchronously and the previously focused card is gone, restore to the same *index* in the same row, else the same row's first item, else the screen's default. Never to "whatever is first in the DOM".

### 3.5 Latency budgets

- **< 100 ms** from keypress to visible focus change, or the UI stops feeling directly manipulated (NN/g's 0.1 s limit). This is a *hard* budget: the focus transition animation must therefore be ≤ ~120 ms and must not gate the state change.
- **< 1 s** for a screen transition to feel like free navigation (NN/g).
- **> 1 s** needs a visible busy state; **> 10 s** needs percent-done and a cancel path (NN/g).
- Corollary for consoles: **route transitions should be skeletons, not spinners**, and focus should land immediately on the skeleton's first item so the user can keep moving.

### 3.6 Input parity

The same action must be reachable by controller, remote, and keyboard — not "mostly". Xbox documents that *some* cloud titles support mouse and keyboard, which is honest scoping; the failure mode is a UI where the controller can open a modal but only a pointer can close it.

Parity checklist for Console 2.0:
- Every focusable thing is reachable by D-pad **and** arrows/Tab.
- Back is available on: gamepad B/Circle, remote Back (Tizen `10009`), keyboard `Esc`/`Backspace`.
- Context actions are on a **button**, not hover.
- Text input has a non-pointer path (on-screen keyboard) even if you normally use the native one.

### 3.7 Interruption and recovery

Consoles win on this and it is under-copied:

- **Suspend/resume** is a product feature, not an OS detail. Steam Deck resumes a game mid-frame; Xbox Quick Resume keeps several. For Ludex, the equivalent is: resume the *shell* to where the user was (row, index, scroll offset) and resume *the game* via the existing launch pipeline, with a clear "still running / relaunched" distinction.
- **Never lose the user's place on error.** A failed row load must not reset the screen.
- **Resume after input loss**: if a controller disconnects, do not reset focus; show a non-modal toast and keep the current focus. If it reconnects, do not move focus.
- **Idle**: after N minutes with no input, dim and show a "still watching?" prompt rather than letting a static UI burn an OLED panel.

### 3.8 Modals

Android TV's layout templates enumerate the overlay types worth having: **center overlay** (urgent decision), **bottom sheet** (contextual actions), **left overlay** (nav), **right overlay** (item actions). Use them like this:

- Modal ⇒ focus trap, `Esc`/B closes, focus returns to the **invoking element** (exact element, not "somewhere in the row").
- Bottom sheet ⇒ non-blocking for the background but still traps focus while open.
- Full-screen "settings app" ⇒ almost always wrong; PS5's Control Center shows the cheaper pattern.
- Confirmations ⇒ default focus on the **safe** action, and never on a destructive one.

### 3.9 Text input / on-screen keyboard

The honest answer: **avoid it.** On a Tauri desktop shell, use the native dialog (`window.prompt` is not acceptable; a Tauri dialog plugin is). In the web app, if search is unavoidable:

- One field, auto-focused, with the caret visible and the label above it.
- `inputmode="search"`, `enterkeyhint="search"`, `autocapitalize="off"`, `spellcheck="false"`.
- Grid OSK with D-pad traversal, **long-press backspace to clear**, and a visible "done" that is always reachable.
- Never require typing to reach the library — filtering, sorting and first-letter jump must work without text entry.

### 3.10 Error, empty, offline

Three distinct states, never one generic "something went wrong":

- **Empty** (valid, zero items): explain *why* and offer the one action that fixes it ("No games yet — add a library folder"). Empty is a design surface, not an error.
- **Offline / server unreachable**: keep the cached library visible with a "last synced 14:02" stamp; disable only actions that truly need the server; retry with backoff and a manual retry.
- **Error**: one sentence of cause, one action, and a copyable diagnostic id. Never block the shell for a failed row.

### 3.11 Performance is an accessibility feature

Samsung's own optimization guide is blunt about the mechanism: memory is shared, and one greedy app degrades the whole device. On a mini-PC the failure is thermal/GPU, not a kill, but the user experience is the same: dropped frames during focus movement, which reads as "the controller is laggy".

Budget targets for Console 2.0 (opinion, but derived from the numbers above):
- 60 fps during focus traversal; **no frame > 16.7 ms** while moving.
- Decoded artwork resident: **< 120 MB** at 1080p (each 1920×1080 bitmap is ~8.3 MB; each 4K bitmap ~33 MB).
- Compositing layers: **single digits**, never one per card.
- Route settle: **< 300 ms** to interactive.

### 3.12 Reduced motion

Required, not optional: `prefers-reduced-motion: reduce` must (a) remove parallax/scale/lift, (b) shorten or remove page transitions, (c) **disable autoplay previews**, (d) keep opacity-only fades. tvOS's own focus effect is a parallax lift — i.e. the canonical console affordance is the first thing you must be able to turn off. Add an in-app toggle as well, because TV/console browsers often do not expose the OS preference to the page.

---

## 4. Web implementation notes

### 4.1 Spatial navigation: nobody will do it for you

**Status (documented):** the spec exists (CSS Spatial Navigation Module Level 1, editor's draft **2026-05-14**, with `getSpatialNavigationContainer()`, `focusableAreas()`, `spatial-navigation-contain`, `spatial-navigation-action: scroll` and `spatial-navigation-function` all marked **at-risk**) and **no browser implements it**. WICG's polyfill page still says "The spec hasn't been implemented in any browser yet" (page updated 2025-09-09) and the polyfill's own header says *written 2017, updated 2019*.

So the realistic options are:

| Option | Reality check |
|---|---|
| **Write it yourself (~80–150 LOC)** | Chrome's own Modern Web Guidance (last modified 2026-11-04) publishes a complete, correct algorithm. Best fit for Ludex: you already own focus state in React. |
| **Norigin Spatial Navigation** | Mature, MIT, production on Tizen/webOS/Hisense/Vizio/STBs; hooks-based (`useFocusable`), "automatic spatial algorithm" — you point components and it resolves the next target. **No gamepad support documented** — you feed it synthetic key events. Long-lived but not spec-shaped. |
| **react-tv-space-navigation (bamlab)** | Declarative (`SpatialNavigationFocusableView`, `SpatialNavigationScrollView`), React Native-first with web support. Authors report virtualization works to 10k items in a row but **breaks with hundreds of rows**, and React 19 + Expo support is WIP. |
| **`spatial-navigation-polyfill`** | Spec-shaped, useful for prototyping, **unmaintained since 2019**. Do not ship it. |

**The algorithm that actually works** (from Chrome's guidance, which is better than most libraries):

1. Candidate set = focusable descendants of the active container that pass `checkVisibility()`. Hidden elements can report a zero-size rect at (0,0) and must not become accidental targets.
2. Read `getBoundingClientRect()` live per keypress — do **not** cache. User-paced events make a handful of geometry reads cheap, and cached rects go stale on scroll, font load, and artwork arrival.
3. **Direction filter is edge-based, not centre-based**: for `right`, require `candidate.left >= current.right - tolerance`. Centre comparison is the classic bug that makes focus jump rows in asymmetric layouts.
4. **Aligned candidates first**: prefer candidates whose perpendicular extent overlaps the current element. Among those, smallest gap on the nav axis; tie-break on smallest perpendicular centre offset.
5. **Fallback score**: `gap + w * perpendicularCentreOffset`, `w >= 2`. This is what keeps you in the adjacent row when one card is unusually wide.
6. **No candidate ⇒ do nothing and do not `preventDefault()`** so native scrolling still works. Unconditionally swallowing arrows breaks WCAG 1.4.10 (reflow) and makes the page feel broken.
7. Only `preventDefault()` when you actually move focus; then `focus()` + `scrollIntoView({block:'nearest', inline:'nearest'})`.
8. Scope the listener to the container (`composedPath()[0]`, not `target`) so multiple navigable regions coexist and shadow DOM doesn't retarget.
9. Skip interception when a modifier is held, when `defaultPrevented`, or when the innermost target is an editable control (`:read-write, select, input[type=range], [role=slider]`).

**Accessibility pairing (required):** roving `tabindex` — active item `0`, all others `-1` — plus composite roles (`grid`/`gridcell` for rows of cards, `listbox`/`option` for selectable lists, `tablist` for content-switching tabs). Chrome's guidance is explicit that `tabindex` alone is not enough for screen readers.

**Edge cases you will hit and must decide up front:**

- **Row wrap at the edge**: right at the last card → stay (console convention) vs wrap to the first (web convention). Pick *stay* for the spine, *wrap* for rows, and document it.
- **Uneven rows**: last row shorter than the one above; up/down must not strand focus. Use the "nearest aligned candidate, else weighted fallback" rule, never `index ± columnCount`.
- **Virtualized lists**: focus target may not be in the DOM. You need a *virtual focus* concept: keep `focusedId` in state, render the row window around it, then call `focus()` after commit. This is the single hardest part of a console UI on the web.
- **Scroll containers**: decide per container whether the D-pad scrolls it or leaves it (`spatial-navigation-action: scroll` semantics). Default: rows scroll horizontally only when focus is inside them.

### 4.2 Focus restoration

Rules that survive contact with real users:

1. **Identity, not index.** Persist a stable id (`gameId`, `rowId`) per route in a stack. Restore by id; fall back to index; fall back to row head.
2. **Restore only on explicit back**, not on every render. Restoring on forward navigation is how you get "focus jumped to the middle of the page".
3. **Never restore into a removed element.** If the id is gone, Apple's rule applies: when the user is moving with a discrete input and the focused item disappears, pick a neighbour; otherwise hide the indicator rather than guessing.
4. **Don't steal focus on async arrival.** If the user has already moved (or the screen was opened without input), do nothing.
5. **After a modal closes**, return to the invoking element, exactly — not to the row.
6. **On controller reconnect**, do nothing at all.

### 4.3 Gamepad pipeline

Documented facts that constrain the design:

- `navigator.getGamepads()` returns a **snapshot**; you must poll it. MDN's current guidance is explicit: use `gamepad.index` as the stable key and read the latest object inside your update loop rather than holding a reference from `gamepadconnected`.
- Indices are **reused** after disconnect. Key your state by index *and* reconnect on `gamepaddisconnected`.
- `mapping === "standard"` is the only guaranteed layout; otherwise indices are vendor-specific. Keep a fallback and, if needed, a mapping table (SDL_GameControllerDB is the canonical community DB, Zlib, last push 2026-11-14, ~1.8k stars).
- Axis convention: `-1` is "forward/left", `+1` "backward/right" (W3C spec). Stick *up* is negative Y — the classic inverted-navigation bug.
- Buttons are `{pressed, value, touched}`; `value` is analog, so **use `pressed` for navigation edges** and `value` only for analog affordances (e.g. scrub speed).
- Haptics exist (`playEffect` with `dual-rumble`/`trigger-rumble`, max ~5 s recommended) and are rejected when the document is hidden.
- Firefox only exposes gamepads **after a user gesture** on a visible page — so "controller detected" UI must tolerate a silent period.

Recommended pipeline:

```
rAF loop (only while window is focused and visible)
  for each connected pad:
    if pad.timestamp === lastTimestamp[i]: continue      // no new data
    dir = axisToDir(pad.axes[0], pad.axes[1], deadzone=0.35)
    if dir changed and |axis| > engagementThreshold: emit(dir); armRepeat(dir)
    else if dir held and repeatDue(): emit(dir)
    for each nav button: edge-detect with 2-frame hysteresis -> emit(action)
```

Tuning that works in practice (opinion, derived from console behaviour):
- **Deadzone** 0.30–0.40 radial, with magnitude rescaling so the usable range isn't truncated.
- **Engagement threshold** for a direction ≈ 0.5, release ≈ 0.3 (hysteresis prevents chatter).
- **Repeat delay** 350–450 ms, **repeat interval** 90–120 ms. Faster than 90 ms and the UI outruns the user's ability to stop — NN/g's "too fast is also a bug" point.
- **Digital D-pad and analog stick must emit the same events**; a lot of controllers send both, so de-duplicate.
- Emit a single normalized event type (`left/right/up/down/accept/back/menu/context`) and map keyboard/remote onto the same bus. This is the input abstraction that keeps Console 2.0 from growing three parallel handlers.

**Runtime risk to test early (not a documented guarantee):** historically, Gamepad support in **WebKitGTK** (your Linux Tauri backend) has lagged Chromium. Before designing around a controller, verify on your actual target: `navigator.getGamepads()` non-null after a button press, `mapping === "standard"`, `timestamp` advancing, and hot-plug events firing. If WebKitGTK is deficient, the mitigation is a Tauri/Rust-side gamepad reader (gilrs) emitting events into the webview — decide this **before** the UI work, not after.

### 4.4 Controller glyphs

- Glyph sets differ by family: Xbox (A/B/X/Y), PlayStation (Cross/Circle/Square/Triangle), Nintendo (A/B/X/Y but **swapped positions**), Steam Deck (ABXY + trackpads).
- **Never label by letter alone.** Label by *position* + family: "bottom face button" resolves to `A` on Xbox, `Cross` on PlayStation, `B` on Nintendo. This is the single most common localization-of-input bug.
- Detect family from `gamepad.id` heuristically and **let the user override** in settings; `id` strings are unspecified by the spec and vary by driver.
- Show glyphs only for the active input modality, and swap the whole set on input change (last-used-device wins). Mixing "press A / click here" in one hint row is noise.
- Keep a text fallback (`A` / `B`) for unknown pads rather than an empty box.

### 4.5 TV safe areas and overscan

- Android: **5 % margins** — 48 dp sides, 27 dp top/bottom at 960×540 design scale (≈ 58 px / 28 px in their grid table).
- Backgrounds should **bleed past** the safe area, not be clipped to it: "allow partial display of offscreen elements".
- On the web, do not confuse this with `env(safe-area-inset-*)` (notches). Implement it as layout padding you control: `padding-inline: max(3.5vmin, 24px)`.
- Keep critical chrome (breadcrumbs, hints, battery/clock equivalents) inside the safe area, and keep the **focus ring** inside too — a ring clipped at the screen edge reads as a layout bug.
- Overscan is mostly legacy, but "cinema/zoom" picture modes and cheap STBs still crop. Design for 5 %, ship a debug overlay that draws the safe rect.

### 4.6 CSS units and layout stability

- Design against a **fixed 1920×1080 logical canvas** and scale one root value (`html { font-size: calc(...) }` or a `--u` unit). Mixing `vw` typography with `px` spacing produces inconsistent density across 720p/1080p/4K panels.
- Use **`dvh`/`svh`, not `vh`** — mobile/TV WebViews still have dynamic viewport quirks, and `100vh` overshoots when browser chrome exists.
- **`aspect-ratio` + explicit `width`/`height` on every artwork element.** This is the cheapest CLS fix there is. Google/Samsung both push it implicitly; the practical rule is: no image without intrinsic dimensions.
- Reserve space for the focus scale with padding, not with layout-affecting margins — animate `transform` and `outline`/`box-shadow`, never `width`/`top`.
- Avoid `font-display: swap` for UI chrome (it causes a reflow at the worst moment); use `optional` + preload the one UI face, and **subset** it (Samsung's guide notes CJK fonts can reach several MB resident).
- Container queries for row density (3 vs 4 vs 5 cards) beat viewport media queries when the row lives inside a padded shell.

### 4.7 Artwork loading

Samsung's guide gives the numbers that make this non-negotiable: **1920×1080 RGBA ≈ 8.3 MB decoded**, **3840×2160 ≈ 33 MB**, and **a single full-screen compositing layer ≈ 8 MB of GPU memory at FHD**.

Practical recipe:
1. Server-side resize to the rendered size (thumb, card, hero). Never ship a 4K poster into a 196 px slot.
2. AVIF/WebP with `<picture>` or `srcset`; one asset per size bucket, not per pixel width.
3. `decoding="async"` + `img.decode()` before attaching — avoids a synchronous decode on the main thread during a focus animation.
4. `loading="lazy"` + `IntersectionObserver` with a generous root margin (one screen ahead), not "load everything on mount".
5. `content-visibility: auto` + `contain-intrinsic-size` on off-screen sections.
6. Release memory for far-offscreen cards (`removeAttribute('src')`) or virtualize: Samsung says any list > ~100 items should be virtualized.
7. Prefetch the **next** row on idle (`requestIdleCallback`), never the whole page.
8. Cache-bust with content hashes and serve immutable — artwork churn is the main cause of cold-start jank.

### 4.8 Memory and GPU budgets

The hard numbers worth internalizing, even though Ludex is not a Tizen app:

- Samsung requires **total app memory (PSS + swap + GPU + GEM) ≤ 380 MB on UHD, ≤ 300 MB on FHD**, with install ≤ 5 MB and data ≤ 20 MB *recommended*. That is the floor of the market; a console shell should be far leaner than a browser tab.
- **Compositing layers are the silent killer**: a full-screen layer ≈ 8 MB. Never put `will-change`, `translateZ(0)` or `backdrop-filter` on every card (Samsung calls this out by name: "don't slap `translateZ(0)` on every card").
- Turn `will-change` **on right before** an animation and remove it after; a permanently promoted element stays resident.
- Don't stack full-screen layers (overlay + scrim + fade) — that's 24 MB before you draw anything.
- WebGL: assume context loss is possible and handle `webglcontextlost`; a lost context must degrade to a CSS fallback, not a blank console.
- Video: one `<video>` element, reused; multiple simultaneously decoding elements is the fastest way to hit a TV GPU ceiling — and the reason Netflix-style autoplay previews are expensive.

### 4.9 Rendering resolution

Documented, and widely ignored: **LG webOS does not support UHD resolution for web apps** — `resolution` is `1920x1080` (default) or `1280x720`. Switch 2 renders its **menus at 1080p** despite 4K output. Design and render UI at 1080p and let the display upscale; a 4K DOM at 60 fps is a losing trade on anything below a discrete GPU.

---

## 5. Top 10 principles for Ludex Console 2.0

1. **One spine, one body, overlays for everything else.** No third navigable level. (Android TV layout templates: browse / left overlay / center overlay / bottom sheet; PS5 Control Center.) — [Android TV layouts](https://developer.android.com/design/ui/tv/guides/styles/layouts), [PS5 Welcome hub](https://blog.playstation.com/2024/09/12/ps5-system-update-adds-welcome-hub-party-share-personalized-3d-audio-profiles-adaptive-controller-charging-and-more/)
2. **Never move focus without user input** — with Apple's single documented exception for a disappearing item. Restore by stable id, on explicit back only. — [Apple HIG: Focus and selection](https://developer.apple.com/design/human-interface-guidelines/focus-and-selection)
3. **Focus feedback in under 100 ms, or it's broken.** Animate ≤ 120 ms, composite-only properties. — [NN/g response times](https://www.nngroup.com/articles/response-times-3-important-limits/)
4. **Body text ≥ 24 px at 1080p, never below ~18 px.** Apple's tvOS minimums are 29/23/17/12 pt; Android says design at 960×540 and prioritize large type. — [Apple HIG: Typography](https://developer.apple.com/design/human-interface-guidelines/typography), [Android TV typography](https://developer.android.com/design/ui/tv/guides/styles/typography)
5. **Respect 5 % overscan, but bleed backgrounds past it.** 48 dp sides / 27 dp top-bottom at Android's 960×540 scale. — [Android TV layouts](https://developer.android.com/design/ui/tv/guides/styles/layouts)
6. **Server-resize artwork; reserve intrinsic dimensions; virtualize beyond ~100 items.** 8.3 MB per 1080p bitmap, 33 MB per 4K bitmap, 8 MB per full-screen layer. — [Samsung memory optimization guide](https://developer.samsung.com/smarttv/develop/guides/web-app-memory-optimization-guide.html), [Samsung HW resource management](https://developer.samsung.com/smarttv/develop/ai-tv-platform-standards/hw-resource-management.html)
7. **One input bus, three sources, full parity.** Normalize keyboard/remote/gamepad into `left/right/up/down/accept/back/context/menu`; deadzone ~0.35, repeat 400/100 ms; label buttons by position + family, never by letter. — [MDN Gamepad](https://developer.mozilla.org/en-US/docs/Web/API/Gamepad_API/Using_the_Gamepad_API), [W3C Gamepad](https://w3c.github.io/gamepad/), [SDL_GameControllerDB](https://github.com/mdqinc/SDL_GameControllerDB)
8. **Write the spatial algorithm; don't wait for the platform.** Edge-based direction filter, aligned-first, weighted fallback, `preventDefault()` only on a real move. No browser implements the spec (editor's draft 2026-05-14, features at-risk). — [Chrome Modern Web Guidance](https://developer.chrome.com/docs/modern-web-guidance/guides/ui-behaviors/spatial-navigation), [css-spatial-nav-1](https://drafts.csswg.org/css-spatial-nav-1/)
9. **Resume the user, not the app.** Restore row/index/scroll, distinguish "game still running" from "relaunched", and never reset the screen on a failed row or a controller reconnect. (Quick Resume / suspend-resume are the console benchmark.) — [Steam Deck software](https://www.steamdeck.com/en/software), [Steam Deck FAQ](https://www.steamdeck.com/en/faq)
10. **Motion is optional; state is not.** Honour `prefers-reduced-motion`, ship an in-app toggle, disable autoplay previews, and never make a blur/parallax treatment load-bearing — Apple gates Liquid Glass by hardware generation. — [Apple tvOS 26 announcement](https://www.apple.com/newsroom/2025/06/apple-tv-brings-a-beautiful-redesign-and-enhanced-home-entertainment-experience/)

---

## 6. Critical notes specific to Ludex (opinion, clearly labelled)

These are the places where copying a AAA console would actively hurt you:

1. **You have no store, so drop the store-shaped home.** Steam and Xbox home screens are merchandising surfaces. Ludex's home should be *state*: Continue, Recently added, Installed & unplayed, Updates available, Server/disk health. Anything else is decoration that costs D-pad presses.
2. **Recommendations require a corpus you don't have.** With a personal library of tens to hundreds of titles, "For you" is noise. Curated *user* shelves (collections, tags, playtime buckets) are honest and cheap.
3. **Switch's one-row home is a trap at scale.** Copy its restraint (two presses to anything) but not its structure: Ludex needs search/filter/sort entry points that Switch deliberately omits.
4. **Plex's 2025 redesign is the anti-pattern to study.** A unified, artwork-dense redesign shipped across clients produced reported TV performance and low-RAM regressions. Console 2.0 should ship *behind* the existing shell, with the old path kept until the new one is measurably faster.
5. **Your worst runtime is not Chrome.** Tauri on Linux = WebKitGTK: weaker gamepad support, different compositing, different video. Budget a spike to test controller input, `backdrop-filter`, and 60 fps row scrolling on WebKitGTK *before* committing to a visual direction. If gamepad is broken there, the Rust-side fallback changes the architecture.
6. **Deep-linking is a console feature.** "Resume this game" from outside the shell (Discord/Steam-like handoff, your existing Sunshine publishing) must land on the running-session state, not on the library grid. Xbox and PlayStation both treat a launch as a state transition, not a navigation.

---

## 7. Sources

All accessed **2026-11-16**.

**Standards and platform guidance**
- Apple, *Designing for tvOS* — https://developer.apple.com/design/human-interface-guidelines/designing-for-tvos (HIG page metadata: modified 2026-11-10)
- Apple, *Focus and selection* — https://developer.apple.com/design/human-interface-guidelines/focus-and-selection
- Apple, *Typography* (tvOS minimum sizes 29/23/17/12 pt) — https://developer.apple.com/design/human-interface-guidelines/typography
- Apple, *Layout* — https://developer.apple.com/design/human-interface-guidelines/layout
- Android, *Design for TV* (10ft, D-pad feedback, communal device) — https://developer.android.com/design/ui/tv/guides/foundations/design-for-tv (modified 2023-05-08)
- Android, *TV layouts* (960×540, 5 % overscan, 12-col/52 dp/20 dp, card widths, templates) — https://developer.android.com/design/ui/tv/guides/styles/layouts (modified 2025-05-09)
- Android, *TV typography* — https://developer.android.com/design/ui/tv/guides/styles/typography (modified 2026-07-06)
- Android, *Compose for TV* — https://developer.android.com/training/tv/playback/compose (modified 2026-10-28)
- Samsung, *Remote Control* (key codes, `10009` back, `registerKeyBatch`) — https://developer.samsung.com/smarttv/develop/guides/user-interaction/remote-control.html
- Samsung, *Web App (HTML5) Memory Optimization Guide* — https://developer.samsung.com/smarttv/develop/guides/web-app-memory-optimization-guide.html
- Samsung, *Hardware Resource Management* (380/300 MB limits, 5/20 MB flash) — https://developer.samsung.com/smarttv/develop/ai-tv-platform-standards/hw-resource-management.html
- LG, *appinfo.json* (1080p-only web apps, `requiredMemory`, `cloudgame_active`, `disableBackHistoryAPI`, `handlesRelaunch`) — https://webostv.developer.lge.com/develop/references/appinfo-json
- W3C, *Gamepad* spec (10-foot scope, axes conventions, haptics) — https://w3c.github.io/gamepad/ (modified 2026-11-14)
- MDN, *Using the Gamepad API* — https://developer.mozilla.org/en-US/docs/Web/API/Gamepad_API/Using_the_Gamepad_API (modified 2026-04-13)
- W3C CSSWG, *CSS Spatial Navigation Level 1* — https://drafts.csswg.org/css-spatial-nav-1/ (editor's draft 2026-05-14)
- WICG, *Spatial Navigation* (spec unimplemented; polyfill 2017/2019) — https://wicg.github.io/spatial-navigation/ (modified 2025-09-09)
- Chrome, *Spatial Navigation (Directional Keypad Focus)* — https://developer.chrome.com/docs/modern-web-guidance/guides/ui-behaviors/spatial-navigation (modified 2026-11-04)
- NN/g, *Response Times: The 3 Important Limits* — https://www.nngroup.com/articles/response-times-3-important-limits/ (1993; updated 2024-01-31)

**Spatial navigation libraries**
- Norigin, *Norigin-Spatial-Navigation* (MIT; Tizen/webOS/VIDAA/Vizio; no documented gamepad support) — https://github.com/NoriginMedia/Norigin-Spatial-Navigation
- bamlab, *react-tv-space-navigation* (declarative; virtualization limits; React 19 WIP) — https://github.com/bamlab/react-tv-space-navigation
- Playnite, *Fullscreen themes docs* (moved/404 at the URL tried; use https://playnite.link/docs/) — https://playnite.link
- SDL, *GameControllerDB* — https://github.com/mdqinc/SDL_GameControllerDB (last push 2026-11-14)

**Product behaviour and 2024–2026 changes**
- Valve, *Steam Deck Software* ("console-like experience… designed specifically for gamepad controls") — https://www.steamdeck.com/en/software
- Valve, *Steam Deck FAQ* ("Will the UI on Steam Deck replace Big Picture? … rollout will happen in stages"; offline play semantics) — https://www.steamdeck.com/en/faq
- Engadget, *Valve's Steam Machine launches in 2026* — https://www.engadget.com/gaming/pc/valves-steam-machine-launches-in-2026-everything-we-know-so-far-200458597.html (2026)
- Steamworks, *New Steam Hardware coming in 2026* — https://steamcommunity.com/groups/steamworks/announcements/detail/563639634283728255 (Nov 2025)
- Apple Newsroom, *Apple TV brings a beautiful redesign…* (tvOS 26, Liquid Glass) — https://www.apple.com/newsroom/2025/06/apple-tv-brings-a-beautiful-redesign-and-enhanced-home-entertainment-experience/ (2025-06-09)
- PlayStation Blog, *PS5 system update adds Welcome hub…* — https://blog.playstation.com/2024/09/12/ps5-system-update-adds-welcome-hub-party-share-personalized-3d-audio-profiles-adaptive-controller-charging-and-more/ (2024-09-12)
- Xbox Support, *Supported browsers for cloud gaming and remote play on the web* — https://support.xbox.com/en-US/help/games-apps/cloud-gaming/supported-browsers
- Xbox Support, *Set up your Windows device for cloud gaming* ("play with a mouse and keyboard instead of a controller while playing some games") — https://support.xbox.com/en-US/help/games-apps/cloud-gaming/setup-cloud-gaming-windows
- Amazon, *Announcing Vega OS* — https://developer.amazon.com/apps-and-games/blogs/2025/09/announcing-vega-os (Sept 2025)
- Amazon Developer Community, *Getting Started with Vega Web Apps and Webview* — https://community.amazondeveloper.com/t/getting-started-with-vega-web-apps-and-webview/20262 (2025-09-30)
- GameSpot, *Switch 2's Main Menu Isn't That Different…* — https://www.gamespot.com/articles/switch-2s-main-menu-isnt-that-different-from-the-one-on-nintendo-switch/1100-6530604/ (2025)
- ResetEra, *Switch 2 renders its menus at 1080p* — https://www.resetera.com/threads/switch-2-renders-its-menus-at-1080p.1208412/ (2025)
- Plex, *It's Go Time: The New Plex Experience Is Here* — https://www.plex.tv/blog/its-go-time-the-new-plex-experience-is-here/ (2025)
- Plex Forums, *New UI is an awful experience* (TV/low-RAM complaints) — https://forums.plex.tv/t/new-ui-is-an-awful-experience/931048?page=23
- Jellyfin, *Blog / releases* (10.11; Android TV 0.20) — https://jellyfin.org/posts/
- GitHub, *Phalcode/gamevault-frontend* (React 19 + Vite + Tailwind 4 + Tauri + Router; stable/unstable updater channels) — https://github.com/Phalcode/gamevault-frontend (last push 2026-11-09)
- Kodi Wiki, *Add-on:Estuary* — https://kodi.wiki/view/Estuary

**Standards referenced but not re-fetched (stable, well-known)**
- WCAG 2.2 quick reference (SC 1.4.3 contrast, SC 1.4.10 reflow, SC 2.3.3 animation from interactions, SC 2.5.8 target size) — https://www.w3.org/WAI/WCAG22/quickref/
- MDN, `prefers-reduced-motion` — https://developer.mozilla.org/en-US/docs/Web/CSS/@media/prefers-reduced-motion
- MDN, `content-visibility` — https://developer.mozilla.org/en-US/docs/Web/CSS/content-visibility

---

### Appendix: verification checklist for the spike (before visual design is locked)

1. WebKitGTK (Linux Tauri build): `navigator.getGamepads()` non-null after button press; `mapping === "standard"` for an Xbox pad; hot-plug connect/disconnect events; `timestamp` advancing.
2. WebView2 (Windows): same, plus `playEffect('dual-rumble')` resolves.
3. 60 fps row scroll with 200 cards, 4 visible, images at 2× DPR, on the weakest target box.
4. Memory ceiling: decoded artwork + layers under 120 MB after 3 minutes of traversal.
5. Focus restoration: back from a detail screen lands on the exact card, after the row has re-sorted.
6. Focus under async load: open a row whose data arrives 800 ms late; focus must not move.
7. Reduced motion: OS preference flips parallax/scale/autoplay off without a reload.
8. Controller disconnect mid-animation: no focus reset, no stuck highlight.
