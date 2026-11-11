# Emulators and ROM libraries (#52)

Ludex can index a folder of emulator ROMs next to (or instead of) game
archives. It does not ship, download or emulate anything itself: the user
registers the command that starts their own emulator, and Ludex only builds an
argument vector and spawns it.

## Library roots

Add a root with `kind` set to `rom`:

```bash
curl -X POST http://ludex:8080/api/library/roots \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"label":"Retro","path":"/mnt/retro","kind":"rom"}'
```

A ROM root is walked at most five levels deep and indexes files whose extension
is one of the platforms in `ludex_core::rom::PLATFORMS` (`nes`, `sfc`, `gba`,
`n64`, `cue`, `chd`, `iso`, `gdi`, `wbfs`, `nsp`, `zip` for arcade, …).
`.bin` and `.iso` tracks are skipped when a `.cue`/`.chd`/`.gdi` descriptor
with the same stem exists. Region and revision tags are ignored for grouping:
`Chrono Trigger (USA).sfc` and `Chrono Trigger (Japan).sfc` are one game with
two files.

The root kind cannot be changed while the root holds indexed versions; this
avoids turning archive rows into ROM rows by accident.

## gamelist.xml

A `gamelist.xml` at the root of a ROM library is read on every scan. The
supported tags are `path`, `name`, `desc`, `image`, `releasedate`, `developer`,
`publisher`, `genre` and `players`. Local images are copied into the media
cache; `http(s)` images are downloaded through the same media cache as provider
artwork. Local gamelist values never overwrite metadata that was set manually
or by a provider (`COALESCE` updates).

## Emulator registry (server)

Administrators register emulators per platform; a platform default and a
per-game override are both supported. A command template must contain `%ROM%`:

```json
{
  "name": "RetroArch · SNES",
  "command": "retroarch -L snes9x_libretro.so %ROM%",
  "platforms": ["snes"]
}
```

Other placeholders: `%ROM_DIR%`, `%ROM_NAME%`, `%ROM_STEM%`. The command is
tokenised in `ludex_core::rom::build_launch_command`, quotes are respected, and
the following are rejected because they only make sense in a shell: `;`, `|`,
`&&`, `$(`, backticks, `>` and `<`. Ludex never invokes `sh -c`.

The server resolves the command for a ROM game with
`POST /api/games/{id}/launch-command`. The desktop app calls this only to get
the ROM path and platform, then launches its **local** registry entry, because
the ROM file may live on another machine.

## Desktop registry

`{appDataDir}/emulators.json` holds local emulators and platform defaults. The
Settings → Emulators panel edits it; built-in presets can be disabled but not
deleted. `launch_rom_game` spawns the resolved command with
`std::process::Command`, records a normal play session and a background thread
closes it when the emulator exits. `stop_rom_game` kills the process tree.

Presets are examples, not guarantees: RetroArch core filenames and standalone
binary names differ per installation. Edit the command to point at your own
binary and core path.

## Limitations

- No emulator binary, BIOS or ROM is distributed with Ludex.
- Disc-based systems rely on the descriptor file (`.cue`, `.gdi`, `.chd`);
  multi-disc games are separate entries unless they share a name.
- A remote desktop client cannot read a server-side ROM path directly. Use
  local mode, a local copy of the ROM, or a shared mount.
- Netplay, per-core configuration and shader presets are out of scope.
