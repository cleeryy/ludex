# Libraries on removable and network mounts

Ludex assumes nothing about where a library lives. A root can sit on the root
filesystem, an external SSD, an NFS export or an SMB share (#290). This
document covers how the server and the desktop app decide a root is usable,
what happens when it goes away, and how to get it back without losing the
index.

## The two sides of a library

| Side | What a "library" is | Where mounts matter |
|---|---|---|
| Server | a row in `library_roots` (label + absolute path) | scanning, uploads, extracted cache |
| Desktop | an install/download/temp *location* (`Settings → Locations`) | downloads, extraction, moves |

Both sides share one decision engine (`ludex_core::mount`), so a NAS export
behaves the same whether it is scanned by the server or installed into by the
desktop app.

## How a root is resolved

Before walking a root (server) or writing into a folder (desktop), Ludex
resolves the mount that backs it from `/proc/self/mountinfo` and takes the
**deepest mount point that is an ancestor of the path**. Component boundaries
are respected: `/mnt/seagate2` never resolves to a `/mnt/seagate` mount.

A root is **online** when the folder exists and the mount that should serve it
is mounted. It is **offline** when either:

- its configured mount (fstab entry or `.automount` unit) is absent, even if
  the mount point directory still exists on the root filesystem; or
- the folder itself is missing on a healthy mount.

An empty mount point is the dangerous case: scanning it would produce zero
files and flag every game as missing. Ludex refuses to scan it and marks the
root offline instead.

### Automount

When the mount is absent, Ludex tries **once** to wake it:

1. a systemd `.automount` unit whose name matches the escaped mount point
   (`/mnt/nas` → `mnt-nas.automount`) is started with `systemctl start`;
2. otherwise, an `/etc/fstab` entry for a mount point above the root is
   mounted with `mount <mount-point>` — but only when its options say the mount
   is automountable: `_netdev`, `noauto`, `x-systemd.automount` (or any
   `x-systemd.automount=` variant).

The mount table is re-read after the attempt. If the mount is still absent, the
root is marked offline and the attempt's error is kept in `last_error`.
There is no retry loop: the next scan (or an explicit **Scan** / **Check**
click) tries again.

Auto-mounted fstab entries on Linux usually look like this:

```
nas.local:/export/games  /mnt/nas/games  nfs4  _netdev,x-systemd.automount,x-systemd.idle-timeout=600,noauto  0  0
```

`_netdev` tells systemd the mount needs the network; `noauto` keeps boot from
blocking on an unreachable NAS; `x-systemd.automount` generates the
`mnt-nas-games.automount` unit. SMB shares work the same way with `cifs`
(credentials in a root-only file, never in the unit).

## Offline behaviour

A root is never deleted because its mount went away.

**Server**

- `library_roots.offline = 1`, `available = 0`, `last_error` carries the
  human message.
- Every version under the root is flagged `root_unavailable`, **never**
  `missing`. Downloading, launching and metadata enrichment skip it; the game
  page shows "storage offline" instead of "file deleted".
- Uploads and the extracted cache refuse to write into an offline root with an
  explicit message instead of an `ENOENT`.

**Desktop**

- `list_locations` / `check_location` report the folder as offline with
  `offline` set, `writable = false`, and no free-space figures (the root
  filesystem's numbers would be a lie).
- Starting an install tries the automount first. If it stays offline the click
  fails with the mount message, before anything is written.
- The partial archive and the staging folder stay exactly where they were.

### Coming back without a rescan

A root is fingerprinted by its **mount identity**: the device id of the mount
point plus the mount point's mtime as a marker, stored as
`mount_device_id` / `mount_marker`.

- Root was offline and comes back with the **same** identity → the scanner
  reuses the stored index. No tree walk; versions are unflagged and the root is
  online again in seconds.
- Root was offline and comes back with a **different** identity (different
  disk, different export) → a full scan runs.
- Root stayed online → every scan walks it normally, so new archives are still
  indexed.
- An explicit "scan this root" (`POST /api/library/roots/{id}/scan`) always
  walks the tree (`force = true`).

## Free-space floor

Every library has a **free-space floor**: the amount that must remain free
after an install. The default is **5 GiB**; `0` disables it on a small scratch
volume.

- Server: `library_roots.min_free_bytes` (`PATCH /api/library/roots/{id}` with
  `{"minFreeBytes": …}`), capped at 1 TiB.
- Desktop: `minFreeBytes` on each entry of `settings.locations`, capped at
  1 TiB. `LocalSettings::floor_for_path` picks the deepest location containing
  the path, then the default install location.

Before the first byte, an install checks:

```
required = archive_size × 1.1   (extraction overhead + staging)
free − required ≥ floor
```

If not, the job is refused with the **missing gigabytes** in the message, e.g.

```
/mnt/nas/games is below its free-space floor of 5.0 GiB: this install needs
12.4 GB more free space (41.0 GiB required including the floor, 28.6 GiB
available). Free space or move the library to a larger volume, then resume.
```

The extraction stage keeps its own, stricter estimate (archive × 1.5 plus a
safety margin) and now applies the same floor, so unpacking can never fill the
volume down to the last byte.

Uploads assemble into the server's ingest root and apply
`archive × 1.1 + floor` there; the extracted cache applies the same rule before
starting an extraction.

## Unplugging a drive mid-download

A write that fails because the volume disappeared (`ENOENT`, `EIO`, `ENXIO`,
`ENODEV`, `ESTALE`, `ERROR_NOT_READY`, "Stale file handle", "Transport endpoint
is not connected") is not treated as a corrupt download:

1. Ludex checks whether the destination mount is offline;
2. if it is, the job moves to **paused** with an explicit message —
   "The library is offline. … The partial archive was kept at …; reconnect the
   drive or share and resume the install.";
3. replugging the drive and pressing **Resume** continues with an HTTP `Range`
   request from the last flushed byte. The staging folder and the `.part` file
   are never rewritten or discarded.

An ordinary missing file on a healthy mount keeps its normal error: the offline
message only appears when a *configured mount* is absent.

## Moving an install to another library

`move_install` copies, verifies, swaps and cleans:

1. **Preflight** — the target root's mount is resolved (automount attempt
   included), and `copy size + floor` must fit on the destination.
2. **Journal** — a row in `install_moves` records source, destination and the
   deterministic staging folder (`.<version>.ludex-move` next to the
   destination) before the first file is written.
3. **Copy** — resumable: files already in staging with the right size are kept,
   so an interrupted move continues instead of starting over. The source is
   never touched.
4. **Verify** — the staging tree is compared with the install manifest
   (per-file SHA-256). A mismatch deletes staging and leaves the game exactly
   where it was.
5. **Swap** — `rename(staging → destination)`, then the index row, manifest
   path and executable are rebased; only then is the source removed. A rename
   failure keeps both copies.
6. **First launch** — the moved install is marked `verify_state = pending`.
   The next launch runs a "Verifying files…" pass against the manifest before
   the game starts; damage stops the launch with a Repair suggestion instead of
   running a half-copied game.

If the app dies mid-move, the journal is replayed at startup: a finished copy
is verified and swapped, an unfinished one resumes. The game is either at the
old path or at the new one — never half-moved.

## Troubleshooting

**The root shows offline although the folder exists.** The folder is an empty
mount point on the root filesystem and the configured mount is not up. Check
`findmnt <path>` and `systemctl status <escaped-mount>.automount`. Press
**Check** / `POST /api/library/roots/{id}/mount` after fixing the mount.

**The automount never fires.** The unit name must match the escaped mount
point (`systemd-escape --path /mnt/nas` → `mnt-nas`). A hand-written fstab line
needs `_netdev` (or `x-systemd.automount`) for Ludex to try `mount`; without
either flag Ludex assumes the mount is not automountable and leaves the root
offline.

**Everything is offline after a reboot.** `noauto` prevents boot blocking, but
the `.automount` unit still has to exist. Check
`ls /run/systemd/generator/*.automount` after `systemctl daemon-reload`.

**A move says the destination exists.** The staging folder from an interrupted
attempt is safe to delete when no move is running:
`rm -rf <target>/.<version>.ludex-move`. The source is untouched by that
command.

**A moved game asks to verify on every launch.** Verification clears
`verify_state` after a successful pass. If it keeps coming back, the manifest
is missing or the tree is changing under Ludex (a sync client or the game
writing next to its executable): run **Verify** from the game page to see the
damaged entries.

**Free-space message despite an empty drive.** Check the library's floor
(`minFreeBytes`): a 5 GiB floor on a 4 GiB USB stick refuses every install by
design. Set the floor to `0` for that library or use a larger volume.

## API surface

| Method | Path | Notes |
|---|---|---|
| `GET` | `/api/library/roots` | includes `offline` and `minFreeBytes` |
| `POST` | `/api/library/roots` | accepts `minFreeBytes` (default 5 GiB) |
| `PATCH` | `/api/library/roots/{id}` | `minFreeBytes`; a path change resets the identity |
| `POST` | `/api/library/roots/{id}/scan` | forced walk |
| `POST` | `/api/library/roots/{id}/mount` | re-check + automount attempt |

Error code for a refused preflight: `insufficient_space` with HTTP `507` and a
message naming the missing gigabytes.
