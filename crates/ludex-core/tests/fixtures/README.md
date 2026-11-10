# Archive fixtures

Binary fixtures used by the `ludex-core` archive tests. They are committed
because the test suite cannot build RAR archives at runtime: `unrar` (and the
`unrar` crate) only reads them.

| File | Contents |
| --- | --- |
| `rar4.rar` | RAR4 (created with RAR 6.24, `-ma4 -m0`). Entries: `Game/readme.txt` ("hello"), `Game/sub/game.sh` (`#!/bin/sh\n`, mode 0755), `Game/unicode-é.txt` ("café"), `Game/empty.txt` (0 bytes), plus the `Game` and `Game/sub` directory entries. |
| `rar5.rar` | Same entries and modes, RAR5 (`-ma5 -m0`). |
| `rar5-encrypted.rar` | RAR5 with file encryption (`-p ludex-test`): names are visible, data is not. |
| `rar5-encrypted-headers.rar` | RAR5 with header encryption (`-hp ludex-test`): even the listing needs a password. |
| `rar4-symlink.rar` | RAR4 holding a single Unix symlink `Game/evil.lnk -> ../../etc/passwd` (`-ol`). |

## Regenerating

RAR 7.x can no longer create RAR4 archives; use RAR 6.24 (trial build from
rarlab.com) on Linux:

```sh
# from a directory containing Game/readme.txt, Game/sub/game.sh,
# Game/unicode-é.txt and Game/empty.txt (with the modes listed above)
rar a -ma4 -m0 rar4.rar Game
rar a -ma5 -m0 rar5.rar Game
rar a -ma5 -m0 -p'ludex-test' rar5-encrypted.rar Game
rar a -ma5 -m0 -hp'ludex-test' rar5-encrypted-headers.rar Game
ln -s ../../etc/passwd Game/evil.lnk
rar a -ma4 -m0 -ol rar4-symlink.rar Game/evil.lnk
```

All fixtures are a few hundred bytes on purpose: the server stream test copies
`rar5.rar` and appends random padding to exceed the scanner's 1 MiB minimum.
libunrar stops at the RAR end-of-archive block, so trailing bytes are ignored.
