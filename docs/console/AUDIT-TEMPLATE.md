# Console mode UI/UX — external audit template (#284)

**Status: awaiting external reviewer pass.** This file is the unfilled report
skeleton and 10-foot heuristic rubric for
[#284](https://github.com/cleeryy/ludex/issues/284). It must be completed by an
external evaluator (someone who did not build the shell) against a **shipped
build** (`v0.5.1` or newer) on **real hardware** (a TV or a 1080p capture card
with a physical controller) — never from screenshots alone and never from a dev
server.

## Reviewer constraints

- Run against the shipped desktop build (`v0.5.1+`), launched in Console mode
  (`ludex --console` or via Sunshine/Moonlight).
- Test with a physical controller on a TV or 1080p capture at couch distance
  (~3 m).
- Record **reproducible steps only**, never ungrounded opinions (for example:
  *"From Stage, open Quick Actions (Start), press B twice → focus disappears"*).
- Do not duplicate host/session findings that belong to `#282` or abstraction
  findings that belong to `#283`; tag them in the priority table instead.

## Current contract to challenge

Read these only after (or alongside) the first-run pass so the initial
impression stays unprimed:

- [Blueprint & decisions](README.md) — `/console/*` shell, single input owner,
  `#333` ARC × Swiss tokens, 4 rooms × 8 knobs, Stage/Wall/List compositions.
- [Automated evidence & visual baselines](evidence.md) — fake-pad E2E,
  720p/1080p/4K + 5% overscan pixel gate (`docs/assets/screenshots/console/`).
- [Performance probe](PERFORMANCE.md) — boot, rail scroll frame times, heap,
  rAF loops.
- [WebKitGTK verdict](WEBKITGTK.md) — Gamepad API behaviour on Linux (`#305`).
- [Hardware & streaming checklist](NATIVE-CHECKLIST.md) — Windows +
  Sunshine/Moonlight sign-off (`#306` / `#270`).
- Copy & settings contract (`#299`): three-tier settings model; no `PROTON_*`,
  no `winetricks`, and no raw exit codes in tiers 1–2.

---

## Session metadata

| Field | Value |
|---|---|
| Reviewer | |
| Date | |
| Shipped build / version | |
| Host OS & display path (direct TV / Sunshine+Moonlight) | |
| Resolution & refresh rate | |
| Viewing distance & screen size | |
| Physical controller(s) used | |
| Recording link / attachment | |

---

## 1. Heuristic pass (10-foot UI checklist)

Mark each criterion `Pass`, `Caveat` or `Fail` and record exact reproduction
steps for every `Caveat` or `Fail`.

| ID | Category | Criterion | Result | Press count / measurement | Reproducible steps & notes |
|---|---|---|---|---|---|
| H1 | Legibility (3 m) | Body/action labels readable at 3 m without squinting (`24u` floor, `28u` body, `48u+` hero) | ☐ Pass ☐ Caveat ☐ Fail | | |
| H2 | Legibility (3 m) | Contrast holds across Night, Projector, Daylight and Arcade rooms (no washed-out text or covers) | ☐ Pass ☐ Caveat ☐ Fail | | |
| H3 | Legibility (3 m) | 5% TV overscan safe band respected — no title, hint or focus ring clipped at screen edges | ☐ Pass ☐ Caveat ☐ Fail | | |
| H4 | Focus & navigation | Focus ring is unmistakable at a glance and has exactly one visible owner on every screen | ☐ Pass ☐ Caveat ☐ Fail | | |
| H5 | Focus & navigation | D-pad / left stick / bumpers / triggers behave consistently across Home, Library, Game, Downloads, Settings and Power | ☐ Pass ☐ Caveat ☐ Fail | | |
| H6 | Press budget | Cold boot → launch last-played game | ☐ Pass ☐ Caveat ☐ Fail | ___ presses | |
| H7 | Press budget | Launch → open overlay → return to game | ☐ Pass ☐ Caveat ☐ Fail | ___ presses | |
| H8 | Press budget | Switch from running game to another installed game | ☐ Pass ☐ Caveat ☐ Fail | ___ presses | |
| H9 | Press budget | Quit game or quit Console cleanly (confirm defaults to Cancel) | ☐ Pass ☐ Caveat ☐ Fail | ___ presses | |
| H10 | Dead ends | Pressing B from every modal, sheet, surface and sub-tab returns focus to the exact origin target | ☐ Pass ☐ Caveat ☐ Fail | | |
| H11 | Dead ends | No route escapes to desktop chrome, toasts, onboarding popups or mouse-only dialogs | ☐ Pass ☐ Caveat ☐ Fail | | |
| H12 | Error & waiting states | Failed launch or game crash returns to Console with clear, actionable copy (no raw exit codes / `PROTON_*` in tiers 1–2) | ☐ Pass ☐ Caveat ☐ Fail | | |
| H13 | Error & waiting states | Network loss / offline mode keeps installed games playable without toast storms | ☐ Pass ☐ Caveat ☐ Fail | | |
| H14 | Controller-only text | Search / sign-in / filter flows are completable with a controller alone (OSK usability, backspace, submit, cancel) | ☐ Pass ☐ Caveat ☐ Fail | | |

---

## 2. First-run journey recording

Record end to end from a **fresh install** to **first game launched**, noting
every screen transition and every waiting state.

| Step | Screen / surface | Waiting state observed | Friction / confusion (with timestamp) | Repro steps |
|---|---|---|---|---|
| 1. Fresh boot | | | | |
| 2. Sign-in / server connection | | | | |
| 3. Empty or first library view | | | | |
| 4. Finding a game (rail / Library / Search) | | | | |
| 5. Opening Game surface | | | | |
| 6. Installing / monitoring Downloads | | | | |
| 7. Launching the game (launch plate) | | | | |
| 8. Opening and closing the in-game overlay | | | | |
| 9. Exiting the game back to Console | | | | |
| 10. Adjusting a Console setting (Room / Theme / Controller) | | | | |

---

## 3. Gap list vs reference experiences

Compare Ludex Console against **Steam Big Picture**, **Nintendo Switch home**
and **PS5 home**. For every gap, explicitly decide whether Ludex should
**Copy**, **Adapt** or **Refuse** it (and why).

| Area | Reference (Steam / Switch / PS5) | Observed gap in Ludex | Verdict (`Copy` / `Adapt` / `Refuse`) | Rationale |
|---|---|---|---|---|
| Home composition & shelf rhythm | | | | |
| Game detail page & launch feedback | | | | |
| In-game overlay & quick actions | | | | |
| Downloads & notifications | | | | |
| Controller-only text input & search | | | | |
| Motion, sound & haptics | | | | |
| Power, sleep & session handoff | | | | |

---

## 4. Redesign directions

Propose **one or two concrete directions** covering the three primary surfaces
plus the controller-only interaction model.

### Direction A — `<title>`

- **Home (Stage / Wall):**
- **Game page:**
- **In-game overlay:**
- **Controller-only interaction & text input model:**
- **Trade-offs & risks:**

### Direction B — `<title>` *(optional if Direction A is decisive)*

- **Home (Stage / Wall):**
- **Game page:**
- **In-game overlay:**
- **Controller-only interaction & text input model:**
- **Trade-offs & risks:**

---

## 5. Priority table

Map every reproducible finding to an existing issue (`#282` host integration,
`#283` abstraction, `#270`/`#306` v0.5 gate) or a new child issue under `#284`.

| Finding ID | Severity (`P0`–`P3`) | Summary | Repro ref | Bucket (`#284` child / `#282` / `#283` / other) | Issue link |
|---|---|---|---|---|---|
| F1 | | | | | |

---

## 6. What we are deliberately not doing (1-page guardrail)

List explicit non-goals so the chosen direction survives contact with the
backlog (e.g., store/social clutter, desktop parity inside the 10-foot shell,
per-game micro-knobs in tier 1, live multi-layer blur).

- **Non-goal 1:**
- **Non-goal 2:**
- **Non-goal 3:**

---

## Acceptance checklist (for closing #284)

- [ ] Completed report merged under `docs/console/` (replacing or archiving this
      template).
- [ ] Every reproducible finding mapped to an existing issue or filed as a child
      issue of `#284`.
- [ ] "What we are deliberately not doing" section completed and reviewed.
