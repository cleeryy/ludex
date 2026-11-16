---
name: console-mode-ui
description: Design and build a 10-foot, gamepad-first Console Mode UI/UX for Ludex.
---

# Console Mode UI/UX Rules

## Core Experience ("Launch & Play")
- Boot directly into the main Game Shelf / Hero view with the most recently played game focused.
- Zero required setup: use sensible defaults for artwork, sorting, and controller bindings.
- Tuck all tweaking/configuration into a clean, optional Settings & Power drawer.

## 10-Foot Visual Hierarchy
- Typography: Minimum 28px body/metadata, 48px+ hero titles at 1080p.
- Contrast: Target 7:1 text contrast on near-black surfaces; use luminance steps for elevation instead of drop shadows.
- Density: Max 1–2 rows visible at once (Hero banner + horizontal game shelf).

## Navigation & Input Parity
- Full D-pad, analog stick, and keyboard arrow-key spatial navigation.
- Always render a persistent bottom controller hint bar (e.g., A: Launch, B: Back, X: Details, Y: Filter, Menu: Settings).
- Focus state must be unmistakable (1.05x scale + high-contrast accent ring + smooth 150ms transition).
- Never trap focus inside modals; B / Escape must always return cleanly to the previous focus target.