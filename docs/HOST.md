# Host preflight

What Ludex checks on this machine before a Windows game is launched, what it
can install with your permission, and what stays a manual step. Related:
[LINUX-GAMING.md](LINUX-GAMING.md) (launch modes, Proton and UMU),
[INTEGRATIONS.md](INTEGRATIONS.md) (compatibility detection) and
[DESKTOP-INSTALL.md](DESKTOP-INSTALL.md) (the AppImage).

## Why this exists

On a clean machine, "Make it run" is a question the user cannot answer: nobody
knows off-hand whether they need UMU, a Proton build, 32-bit Vulkan or
`libwebkit2gtk-4.1`. The preflight answers that first: it probes the stack once,
classifies every finding as **missing**, **too old**, **ready** or **unknown**,
and gives each finding one human sentence ("your GPU supports Vulkan, the
32-bit runtime does not").

The result is stored in `settings.json` (`hostPreflight`), so a launch or a
second window reads the stored report instead of re-probing. The report has a
timestamp (`checkedAt`); **Re-run checks** in Settings → Host is the only action
that probes again and replaces it.

## What is probed

| Finding (`id`) | Source | `ok` | `too old` | `missing` | `unknown` |
|---|---|---|---|---|---|
| Graphics driver (`gpu`) | `/sys/class/drm/card*/device/uevent` (`DRIVER`, `PCI_ID`); `lspci -nn` as a fallback description; `nvidia-smi` as a hint only | a kernel driver is bound to a card, not a firmware framebuffer | `nouveau` is loaded (no vendor Vulkan driver through it) | firmware framebuffer (`simpledrm`, `efifb`, …) or a GPU in `lspci` with no driver | no card and no `lspci` entry |
| Vulkan (`vulkan`) | `/usr/share/vulkan/icd.d/*.json`, `/usr/local/share/…`, `/etc/vulkan/…`; `vulkaninfo --summary` as a hint | a hardware ICD is installed and `vulkaninfo` does not fail | only a software ICD (`lvp`, `lavapipe`, SwiftShader) or `vulkaninfo` fails | no ICD at all | — |
| 32-bit Vulkan (`vulkan32`) | `libvulkan.so.1` in `/usr/lib32`, `/usr/lib/i386-linux-gnu`, …; each `icd.d/*.json` `library_path` resolved in those folders (plus `i686`/`_32` manifest names) | loader **and** a 32-bit-capable ICD, separate or arch-agnostic | 32-bit loader without a 32-bit-capable ICD | 32-bit loader but every ICD manifest is unreadable | no 32-bit loader |
| WebKitGTK runtime (`webkitgtk`) | `libwebkit2gtk-4.1.so.0` in the library folders | the 4.1 runtime is installed | only `libwebkit2gtk-4.0.so.0` | neither | — |
| UMU (`umu`) | the existing compatibility scan (`compat.rs`) plus `umu-run`/`umu-launcher` on `PATH` | found | — | not found | the scan did not run |
| Proton builds (`proton`) | the existing compatibility scan: Steam roots, `compatibilitytools.d`, `steamapps/common` | at least one build | — | none | the scan did not run |
| GameMode (`gamemode`) | `gamemoderun` on `PATH` | found | — | not found | — |
| MangoHud (`mangohud`) | `libMangoHud.so`, including the `mangohud/` subfolder and the 32-bit folders | found | — | not found | — |
| Gamescope (`gamescope`) | `gamescope` on `PATH` | found | — | not found | — |

Proton and UMU reuse `compat::scan` instead of a second implementation, so the
launch options panel and the preflight never disagree. `lspci`, `nvidia-smi` and
`vulkaninfo` are optional: a missing binary only drops the hint, it never fails
the probe. Each command is bounded to 4 seconds and runs off the UI thread.

Outside Linux every finding is `unknown` with a sentence saying that host checks
only apply to Linux; nothing is offered for install.

## Distribution detection

The package family comes from `/etc/os-release` (`ID`, `ID_LIKE`), then from the
package manager binary actually present on `PATH`:

| Family (`id`) | `ID`/`ID_LIKE` seen | Install command |
|---|---|---|
| `pacman` | arch, cachyos, endeavouros, manjaro, garuda, artix | `pkexec pacman -S --needed --noconfirm <packages>` |
| `dnf` | fedora, nobara, bazzite, centos, rhel, rocky, almalinux | `pkexec dnf install -y <packages>` |
| `apt` | debian, ubuntu, linuxmint, pop, zorin, elementary, kali, raspbian, neon | `pkexec apt-get install -y <packages>` |
| `zypper` | opensuse-tumbleweed, opensuse-leap, suse, sles, sled | `pkexec zypper --non-interactive install <packages>` |
| `unknown` | nothing recognized and no `pacman`/`dnf`/`apt`/`zypper` on `PATH` | none; the report explains and keeps the manual instructions |

An unknown distribution never gets an invented command: the Install button is
unavailable, and each finding keeps its official link.

## The recipe table

Package names are data (`RECIPES` in `host_preflight.rs`), one row per finding
with one column per family. A family with no package is marked **manual** with
an official link instead of a guessed name.

| Finding | pacman | dnf | apt | zypper |
|---|---|---|---|---|
| UMU | `umu-launcher` | `umu-launcher` | *no package — manual* | `umu-launcher` |
| GameMode | `gamemode` | `gamemode` | `gamemode` | `gamemode` |
| MangoHud | `mangohud` | `mangohud` | `mangohud` | `mangohud` |
| Gamescope | `gamescope` | `gamescope` | `gamescope` | `gamescope` |
| WebKitGTK 4.1 | `webkit2gtk-4.1` | `webkit2gtk4.1` | `libwebkit2gtk-4.1-0` | `webkit2gtk4.1` |

Ubuntu and Debian do not package UMU, so the apt column is empty on purpose:
the report shows the official release link instead. Proton builds are not
distribution packages at all — the finding points at **Make it run**, which
downloads a build through the runtime catalog.

## Consent flow

1. Settings → Host lists the findings grouped by status; each row carries the
   sentence, the path/version hint, and the exact package names for the detected
   family.
2. **Install missing** opens a dialog built from a **dry run**
   (`host_preflight_install` with `dryRun: true`, or `host_preflight_plan`). It
   lists the packages, the exact command (including `pkexec`), the manual steps
   and any refused ids. Nothing runs while the dialog is open.
3. **Install packages** calls the command with `consent: true`. One command per
   family runs through `pkexec`, so polkit owns the password prompt: Ludex never
   runs as root, never keeps a privileged helper and never builds a shell string.
4. The output (stdout + stderr, bounded to 8 000 characters) is shown in the
   dialog. On success the stored report is cleared so the next
   `host_preflight_report` probes again — a fresh install changes the answer.

Without `consent: true` the backend rejects the call; the UI cannot install by
accident.

## No GPU drivers, ever

Drivers are the one thing this feature refuses to install:

- the GPU finding has no row in `RECIPES`, and `plan_install` refuses the `gpu`
  id by name even when a caller asks for it;
- the UI hides it from the install selection, but the guarantee is in Rust, not
  in the button;
- the installed paths default to the distribution tool ("Additional Drivers" on
  Ubuntu) and link to the Lutris driver guide.

A test asserts that no package in the table contains a driver keyword
(`nvidia`, `nouveau`, `mesa`, `amdgpu`, `i915`, `firmware`).

### 32-bit Vulkan on Arch and CachyOS

Arch and its derivatives ship **one arch-agnostic ICD per vendor** instead of a
32-bit manifest. `nvidia_icd.json` carries the bare soname
`libGLX_nvidia.so.0`, and whichever loader opens it chooses the architecture:
`/usr/lib32/libvulkan.so.1` (package `lib32-vulkan-icd-loader`) resolves the
same manifest to `/usr/lib32/libGLX_nvidia.so.0` as soon as the 32-bit driver
package is installed (`lib32-nvidia-utils` for NVIDIA, `lib32-vulkan-radeon` or
`lib32-vulkan-intel` for Mesa). The probe parses `library_path` and resolves it
in the 32-bit folders, so this setup is **ok**; a file named `*i686*` or
`*_32.json` is no longer required. AMD still ships `radeon_icd.x86_64.json` plus
`radeon_icd.i686.json`, and both paths work.

When the finding is **too old**, check before changing packages:

- `vulkaninfo --summary` with a 32-bit `vulkaninfo` when your distribution
  ships one — the 64-bit command only exercises the 64-bit loader;
- `ldd /usr/lib32/libGLX_nvidia.so.0` (or the library named by the manifest) to
  confirm the 32-bit build is installed;
- `pacman -Qo /usr/lib32/libvulkan.so.1` on Arch (`dpkg -S` on Debian,
  `rpm -qf` on Fedora) to see which package owns the loader.

On CachyOS, an `nvidia-dkms` transaction conflicts with the prebuilt
`linux-cachyos-nvidia-open` modules the running kernel uses, and
`lib32-nvidia-utils` pins `nvidia-utils` to its exact version. The finding
therefore never names driver packages and never offers a command that changes
them: it reports the diagnostics above and the conservative sentence that
32-bit games fall back to OpenGL until a 32-bit ICD is available.

## Re-run and persistence

- First start: `spawn_startup_probe` runs once, off the runtime, and stores the
  report. A launch never re-probes while a stored result exists.
- **Re-run checks** (`host_preflight_report(refresh: true)`) probes again and
  overwrites the stored result.
- A successful install clears the stored result, so the next read probes the
  patched machine.
- The first-run prompt on the desktop shell is dismissible and never blocking.
  The dismissal is stored per `checkedAt`, so a new probe can prompt again.

## What stays manual

- GPU drivers and anything driver-shaped (Mesa, `nvidia`, firmware).
- UMU on Ubuntu/Debian (release zipapp or Flatpak).
- Proton builds: **Make it run** or Steam.
- 32-bit Vulkan when the distribution has no 32-bit driver build; the report
  explains that 32-bit games fall back to OpenGL and offers diagnostics that
  never replace a working driver package.
- A machine where neither `/etc/os-release` nor `PATH` names a known package
  manager.

## Where the code lives

- Rust: `apps/desktop/src-tauri/src/host_preflight.rs`; commands
  `host_preflight_report`, `host_preflight_plan`, `host_preflight_install`;
  the last report is `LocalSettings::host_preflight`.
- Web: `apps/web/src/components/settings/host-preflight-panel.tsx` (Settings →
  Host), `host-preflight-prompt.tsx` (first-run card),
  `apps/web/src/lib/host.ts` and `mock-host.ts` (bridge and demo adapter).
