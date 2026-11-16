# Screenshots gallery

Every image on this page is captured from the shipped web UI running the built-in demo
library — no mockups, no roadmap screens. The README uses a subset; the rest are here for
docs, issues and release notes.

The pipeline and the capture rules are documented in
[`assets/screenshots/README.md`](assets/screenshots/README.md); the machine-readable record of
the last run (version, commit, date, viewport, theme) is
[`assets/screenshots/manifest.json`](assets/screenshots/manifest.json).

## Desktop

### Library — dark and light

The same screen in both built-in themes; the README picks one with `prefers-color-scheme`.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="assets/screenshots/readme/library-dark.png">
  <source media="(prefers-color-scheme: light)" srcset="assets/screenshots/readme/library-light.png">
  <img src="assets/screenshots/readme/library-dark.png" alt="Library grid: covers, install badges, platform and launcher filters, state tabs" width="100%">
</picture>

### Home

![Home: continue playing, installs on this device and recently played](assets/screenshots/docs/home-dark.png)

### Game page

![Game page for HITMAN 3 with cover, rating, size, versions, playtime and the trailer facade](assets/screenshots/readme/game-page.png)

### Downloads

![Downloads queue with one active install, its speed and ETA, and queued jobs](assets/screenshots/readme/downloads.png)

### Console mode

Console mode is the ten-foot shell that Sunshine publishes as a single `Ludex` app. The two
appearance presets share the layout, the navigation and the sound language.

![Console mode, Soft appearance: cream rounded tiles over blurred key art](assets/screenshots/readme/console-soft.png)

![Console mode, Studio appearance: dark sharp archive surfaces over artwork](assets/screenshots/readme/console-studio.png)

### Admin

![Admin games manager with availability, versions, size, installs and VirusTotal verdicts](assets/screenshots/docs/admin-games.png)

![Integrations: VirusTotal key status, daily quota and periodic re-check policy](assets/screenshots/docs/admin-virustotal.png)

![Admin ingest: add a download by URL and watch the active job](assets/screenshots/docs/admin-ingest.png)

### Settings

![Settings: account, appearance, downloads, saves, streaming and integrations](assets/screenshots/docs/settings.png)

## Mobile (PWA)

Captured at a 390×844 viewport, 2× pixel density.

<table>
  <tr>
    <td width="25%"><img src="assets/screenshots/mobile/home.jpg" alt="Phone home screen with continue playing and the tab bar"></td>
    <td width="25%"><img src="assets/screenshots/mobile/library.jpg" alt="Phone library with a two-column cover grid"></td>
    <td width="25%"><img src="assets/screenshots/mobile/game-page.jpg" alt="Phone game page with poster and install button"></td>
    <td width="25%"><img src="assets/screenshots/mobile/downloads.jpg" alt="Phone downloads queue with progress"></td>
  </tr>
  <tr>
    <td align="center"><sub>Home</sub></td>
    <td align="center"><sub>Library</sub></td>
    <td align="center"><sub>Game page</sub></td>
    <td align="center"><sub>Downloads</sub></td>
  </tr>
</table>

## Regenerate

```bash
pnpm screenshots                 # everything, ~2 minutes
pnpm screenshots --only mobile   # one set
node scripts/check-assets.mjs    # budgets and broken references
```
