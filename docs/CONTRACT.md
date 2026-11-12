# Ludex API + desktop contract (v0.1)

Single source of truth for the server, web UI and Tauri shell. All JSON is
`camelCase`. All routes are under `/api` unless stated. Auth is
`Authorization: Bearer <jwt>` or, for integrations, `Authorization: Bearer
ldx_<token>` (an API key) except public routes.

## Errors

```json
{ "error": { "code": "invalid_credentials", "message": "Invalid username or password" } }
```

HTTP status is meaningful: 400 validation, 401 unauthenticated, 403 forbidden,
404 not found, 409 conflict, 422 semantic, 429 rate limited, 500 internal.

## Types

```ts
type Role = "admin" | "editor" | "user";
type ProgressState =
  | "unplayed" | "playing" | "completed" | "abandoned" | "onHold" | "planToPlay";

interface User {
  id: number; username: string; email: string | null; role: Role;
  activated: boolean; avatarUrl: string | null; createdAt: string;
  /** Identity source. `oidc` accounts are linked to an identity provider. */
  source: "local" | "oidc";
  /** Issuer of the linked provider; `null` for local accounts. */
  oidcIssuer: string | null;
  /** Maximum age rating this account may access; `null` = unrestricted (#39). */
  ageLimit: number | null;
  /** When the first-run wizard was completed or dismissed (#119). */
  onboardingCompletedAt: string | null;
  /** Canonical wizard step to resume at (#119), 0..=20. */
  onboardingStep: number;
}

interface GameSummary {
  id: number; title: string; sortTitle: string; releaseDate: string | null;
  coverUrl: string | null; backgroundUrl: string | null; rating: number | null;
  genres: string[]; earlyAccess: boolean; versionCount: number; totalSize: number;
  state: ProgressState; minutesPlayed: number; bookmarked: boolean;
  lastPlayedAt: string | null; maxPlayers: number | null;
  /** `game` for an archive, `rom` for an emulator ROM (#52). */
  kind: "game" | "rom";
  /** Platform slug for a ROM entry (`snes`, `gba`, …); null otherwise. */
  platform: string | null;
  /** Per-game emulator override; null uses the platform default. */
  emulatorId: number | null;
  /** Present only when the row came from a search (`q`). */
  matchedField?: "title" | "sortTitle" | "altTitle";
  matchedTitle?: string; matchScore?: number;
}

interface GameVersion {
  id: number; fileName: string; size: number; version: string | null;
  releaseDate: string | null; earlyAccess: boolean;
  /** Cheap archive fingerprint (`sha256-sample:<hex>`), null until computed. */
  archiveHash: string | null;
  /** Last time the version row changed (indexed, renamed or re-hashed). */
  updatedAt: string | null;
  downloadUrl: string; // /api/games/{id}/download?versionId={id}
}

/** Newest available version, used by desktop clients to detect updates. */
interface LatestVersion {
  gameId: number; versionId: number;
  versionLabel: string | null;   // parsed from the file name
  fileName: string;
  archiveHash: string | null;    // `sha256-sample:<hex>` sampled fingerprint
  archiveSize: number;           // bytes
  updatedAt: string | null;      // RFC 3339
  earlyAccess: boolean;
  downloadUrl: string;
}

interface GameVideo {
  title: string;
  url: string;
  thumbnailUrl: string | null;
  /** YouTube id for IGDB trailers; null for direct file URLs. */
  videoId?: string | null;
  /** "youtube" or "file". */
  provider?: string | null;
}

interface GameDetail extends GameSummary {
  description: string | null;
  screenshots: string[]; // media URLs
  /** Deprecated alias of `videos`, kept for v0.2 clients. */
  trailers: GameVideo[];
  /** Trailers and other videos, with provider metadata. */
  videos: GameVideo[];
  developers: string[]; publishers: string[]; tags: string[]; ageRating: number | null;
  provider: { slug: string; id: string; url: string | null } | null;
  altTitles: string[];
  versions: GameVersion[];
  launch: {
    executable: string | null; parameters: string | null;
    installerExecutable: string | null; installerParameters: string | null;
    uninstallerExecutable: string | null; uninstallerParameters: string | null;
  };
}

interface NewsPost {
  id: number; title: string; body: string; author: string;
  createdAt: string; updatedAt: string;
}

interface IngestJob {
  id: string; title: string; url: string | null;
  status: "queued" | "downloading" | "verifying" | "done" | "error" | "cancelled";
  fileName: string | null; downloadedBytes: number; totalBytes: number | null;
  speedBps: number; attempt: number; error: string | null;
  createdAt: string; updatedAt: string;
}

interface ScanStatus {
  running: boolean; startedAt: string | null; finishedAt: string | null;
  scanned: number; added: number; updated: number; removed: number;
  metadataPending: number; errors: string[];
}
```

## Public

| Method | Route | Response |
|---|---|---|
| GET | `/api/health` | `{ status: "ok", version, uptimeSeconds }` |
| GET | `/api/health/details` | database/disk/library/ingest/watcher/integrity details (`HealthDetails`); public, no paths or secrets |
| GET | `/api/setup/status` | `{ needsSetup: boolean, serverName: string, version: string }` |
| POST | `/api/setup` | body `{ username, password, serverName? }` → `{ token, user }`; 409 if users exist |
| GET | `/api/media/{name}` | cached image/file (public, immutable cache headers) |
| GET | `/api/config` | `{ serverName, version, features: { igdb, steam, ingest, localMode, oidc }, maxUploadBytes, oidcDisplayName }` (public, used by setup/login). `features.oidc` and `oidcDisplayName` reflect the live OIDC client, so a saved admin configuration applies without a restart |
| GET | `/api/desktop/download` | latest stable desktop installers as links (`DesktopDownload`); optional `?platform=windows\|linux\|macos`, see *Desktop download page (#164)* |
| GET | `/api/share/{token}` | public share page (`ShareView`); see *Share links (#9)* |
| POST | `/api/share/{token}/access` | exchange a share link for a 15-minute download ticket; see *Share links (#9)* |

```ts
interface HealthDetails {
  status: "ok" | "degraded";       // degraded when SELECT 1 fails
  version: string;
  commit: string;                  // LUDEX_GIT_COMMIT, or "unknown"
  uptimeSeconds: number;
  database: {
    ok: boolean;
    backend: "sqlite" | "postgres";
    sizeBytes: number;
    connections: number;
    idleConnections: number;
    maxConnections: number;
  };
  disk: { dataFreeBytes: number; gamesFreeBytes: number; mediaFreeBytes: number };
  library: {
    games: number; versions: number; rootsTotal: number; rootsAvailable: number;
    mediaFiles: number; mediaBytes: number;
  };
  ingest: {
    queued: number; running: number; failed: number;
    done: number; cancelled: number; total: number;
  };
  watcher: {
    enabled: boolean; pending: boolean; roots: number;
    lastEventAt: string | null; lastError: string | null;
  };
  integrity: {
    id: number; status: "completed" | "failed"; findings: number;
    startedAt: string | null; finishedAt: string | null;
  } | null;
  storageBackend: "local" | "s3";
}
```

`/api/health/details` never returns filesystem paths, credentials or bucket
keys. It is safe for uptime monitors and dashboards.

## Desktop download page (#164)

`GET /api/desktop/download` powers the public `#/download` page. It is
unauthenticated (a visitor who cannot sign in yet still needs the client),
returns links only, and never proxies a binary: the browser downloads straight
from the release assets. The parsed release document is cached in-process for
ten minutes, so a shared NAT or a busy page does not spend GitHub's
unauthenticated rate limit. The global per-IP API limiter still applies.

| Method | Route | Notes |
|---|---|---|
| GET | `/api/desktop/download?platform=windows\|linux\|macos` | `platform` is optional; omitted lists every asset, an unknown value answers 400 `invalid_platform` |

```ts
interface DesktopAsset {
  platform: "windows" | "linux" | "macos";
  kind: "installer" | "appimage" | "deb" | "rpm" | "dmg";
  name: string;                     // release asset file name
  url: string;                      // https release asset, or the configured mirror
  sizeBytes: number;
  sha256: string | null;            // set when the release publishes SHA256SUMS
}

interface DesktopDownload {
  channel: "stable";
  source: "github" | "mirror" | "cache" | "fallback";
  stale: boolean;                   // cache/fallback answer that could not be refreshed
  version: string | null;           // "0.3.2" (no leading v)
  tag: string | null;               // "v0.3.2"
  releasedAt: string | null;        // RFC 3339
  releaseUrl: string;               // always present, even offline
  notes: string | null;             // first paragraph, plain text, bounded
  checksumsUrl: string | null;      // SHA256SUMS asset when published
  platform: "windows" | "linux" | "macos" | null; // echo of the query
  recommended: DesktopAsset | null; // best installer for `platform`
  assets: DesktopAsset[];
}
```

- Updater artifacts (`*.sig`, `latest.json`, `*.tar.gz`, `*.nsis.zip`) are never
  listed as installers. Windows `.exe`/`.msi`, Linux `.AppImage`/`.deb`/`.rpm`
  and macOS `.dmg`/`.pkg` are.
- `recommended` prefers the one-file installer: `setup.exe`, `.AppImage`, `.dmg`,
  then `.deb`, then `.rpm`.
- **Offline / rate-limited upstream.** The endpoint still answers `200`:
  `source: "cache"` with `stale: true` when a previous answer exists, otherwise
  `source: "fallback"` with no assets and the configured release page in
  `releaseUrl`. A visitor never sees a broken button.
- **No credentials.** The request to the releases API carries no `Authorization`
  header and no token is read from the environment; a public GitHub quota is all
  the feature needs.
- **Checksums.** When the release publishes a release-wide `SHA256SUMS` (or
  `SHA256SUMS.txt`) asset, it is fetched once per cache window and parsed;
  matching assets get `sha256`, and `checksumsUrl` links the file. Per-asset
  `*.sha256` files (the server tarball publishes one) are not treated as a
  manifest.
- **Security.** Only absolute `http(s)` URLs from the upstream are emitted;
  `javascript:`, `data:` and malformed URLs are dropped.
- The SPA route `#/download` lives **outside** the auth gate. It detects the
  visitor's OS for the primary button, lists every platform, and shows a
  "open this address on your PC" card on phones. The in-app "Get the desktop
  app" prompt is client-side only: once per session ("Not now" is stored in
  `sessionStorage`), permanently hidden with "Don't show again"
  (`localStorage`), and never rendered inside Tauri or an installed PWA.

## Auth

| Method | Route | Notes |
|---|---|---|
| POST | `/api/auth/login` | `{ username, password }` → `{ token, user }` |
| GET | `/api/auth/me` | `{ user }` |
| POST | `/api/auth/logout` | 204 (client discards token) |
| POST | `/api/auth/change-password` | `{ currentPassword, newPassword }` |
| PUT | `/api/auth/onboarding` | `{ completed?, step? }` → `{ user }`; self-service wizard preference, see *Onboarding (#119)* |
| POST | `/api/auth/desktop-handoff` | session + `write` scope; `{ state }` → `{ expiresAt }`; binds the calling web session to a desktop sign-in, see *Desktop SSO handoff (#157)* |
| POST | `/api/auth/desktop-handoff/claim` | public; `{ state }` → `{ code, expiresAt }` **once**; 410 `handoff_unavailable` when unknown, expired or used |

### Desktop SSO handoff (#157)

The desktop app cannot receive an OIDC redirect, so the browser hands the
session back through a short-lived, state-bound one-time code. No local port is
opened and no token ever travels in a URL.

1. The app generates a 32-byte state (`43` chars base64url or `64` hex) and
   opens `{serverUrl}/#/login?desktop_state=<state>` in the system browser.
2. The web app keeps the state in `sessionStorage` across the OIDC redirect.
3. After a successful sign-in (SSO **or** password) it calls
   `POST /api/auth/desktop-handoff { state }` once and shows *"Return to Ludex;
   you can close this tab."*. Without `desktop_state`, nothing changes.
4. The app polls `POST /api/auth/desktop-handoff/claim { state }` every second
   for up to five minutes, then redeems the code through
   `POST /api/auth/oidc/exchange` — the only place a JWT is issued.

```ts
interface DesktopHandoff {           // POST /api/auth/desktop-handoff
  expiresAt: string;                 // RFC 3339, ~2 min (LUDEX_DESKTOP_HANDOFF_TTL_SECONDS)
}

interface DesktopHandoffClaim {      // POST /api/auth/desktop-handoff/claim
  code: string;                      // single-use exchange code, never a token
  expiresAt: string;
}
```

- `state` must be 43–128 characters of `A-Z a-z 0-9 - _` and is compared in
  constant time; a state already bound to another account is a `409
  handoff_state_conflict`.
- Creation requires SSO to be configured, the `write` scope, and is limited to
  12 handoffs per account per minute (`429 handoff_throttled` +
  `Retry-After`). Claims are limited to 120 per address per minute.
- Claims answer `410 handoff_unavailable` for unknown, expired and already-used
  states alike: the endpoint never reveals whether a sign-in is pending or for
  whom. The code is removed on claim, so a second claim always fails.
- Pending handoffs live in memory only (bounded to 1024 entries, lazy expiry on
  every access) and die with the process. The code expires with the handoff.
- Audited as `oidc.handoff_created`, `oidc.handoff_claimed` and
  `oidc.handoff_failed` (reason only). Neither the state nor the code is
  recorded.


### Onboarding preferences (#119)

The optional first-run wizard stores two preference fields on the account:

- `onboardingCompletedAt` — set when the account finishes or chooses
  "Don't show again"; `null` while the tour is pending. Sending
  `{ "completed": false }` clears it and reopens the wizard (Settings → Help,
  "Restart onboarding").
- `onboardingStep` — canonical step index, validated to **0..=20**
  (`400 invalid_onboarding_step` otherwise). The canonical order is
  `welcome`, `library`, `downloads`, `saves`, `appearance`, `admin`, `finish`;
  non-administrators skip `admin`, and the server does not interpret the index.

`PUT /api/auth/onboarding` requires an authenticated session (or an API key
with the `write` scope) and always acts on the calling account. Both fields are
omitted from `PATCH /api/users/{id}`: an administrator cannot amend another
account's wizard state through user management, and unknown fields in that
payload are ignored. New and existing accounts default to
`null` / `0`.

## Games (authenticated)

| Method | Route | Notes |
|---|---|---|
| GET | `/api/games` | query: `q`, `genre`, `state`, `bookmarked`, `earlyAccess`, `yearFrom`, `yearTo`, `players`, `playtimeMin`, `playtimeMax`, `minRating`, `provider`, `platform`, `kind` (`game\|rom`), `sort` (`title\|releaseDate\|rating\|addedAt\|lastPlayed\|playtime`), `order` (`asc\|desc`), `page` (1), `limit` (48, max 200) → `{ items, total, page, limit }` |
| GET | `/api/games/suggest` | query: `q`, `limit` (8, max 20) → `{ query, suggestions: [{ id, title, year, coverUrl, matchedField, matchedTitle, score }] }`; top matches, best first |
| GET | `/api/games/{id}` | `GameDetail` |
| GET | `/api/games/random` | same filters as list → `GameSummary`; 404 when empty |
| PATCH | `/api/games/{id}` | editor; editable metadata fields (`altTitles`, `players`) + `launch` object; returns `GameDetail` |
| POST | `/api/games/{id}/refresh-metadata` | editor; 202 `{ queued: true }`. The job fetches the stored provider id when one is set instead of searching by title again (#135) |
| GET | `/api/games/{id}/metadata/candidates` | editor; query `q` (defaults to the game title, 1-120 chars), `provider` (one slug), `year` → `{ query, gameId, manualFields, candidates, providers }`. Network runs only when this route is called; each provider has a 6 s deadline and its own error state |
| POST | `/api/games/{id}/metadata/apply` | editor; `{ provider, providerId, replaceManual? }` → `GameDetail`; caches the record's cover/background/screenshots/trailers, replaces title/year/description/genres/developers/publishers/rating/ageRating/maxPlayers/altTitles, always sets `provider.slug/id/url`, and emits `game.metadata_matched` |
| POST | `/api/games/{id}/download-ticket` | `{ versionId? }` → `{ url, expiresAt }` (15 min, no auth needed on the URL) |
| GET | `/api/games/{id}/download?versionId=&token=` | file stream, Range supported, `Content-Disposition` |
| GET | `/api/games/{id}/versions/{versionId}/stream?token=` | Stream install: an uncompressed `application/x-tar` of the archive entries, no archive stored on the client. ZIP, 7z and RAR (RAR4/RAR5) are supported; unsupported files return 415 and the client falls back, encrypted archives return 409. Exact `Content-Length`. Same auth rules as download. |
| GET | `/api/games/{id}/versions/{versionId}/stream/manifest?token=` | Ordered ZIP manifest for resumable per-entry installs (#115): `{ format: "zip", entryCount, totalBytes, totalCompressedBytes, entries: [{ index, path, size, compressedSize, mode, crc32, isDir }] }`. Same auth as `/stream` (Bearer or download ticket). Non-ZIP → `415 stream_unsupported`. |
| GET | `/api/games/{id}/versions/{versionId}/stream/entries/{index}?token=` | Raw content of one ZIP entry, single HTTP `Range` supported (`206` + `Content-Range`, `416` + `Content-Range: bytes */size`). `Content-Length` is exact. Unknown index → `404 entry_not_found`; directories → `404 entry_not_found`. Same auth as `/stream`. |
| GET | `/api/games/{id}/versions/latest` | newest non-missing version → `LatestVersion`; 404 `no_versions` when the game has none |
| GET | `/api/games/{id}/versions/{versionId}/local-path` | embedded/local mode + loopback only → `{ path }`, else 404 |

### Manual metadata match (#135)

`GET /api/games/{id}/metadata/candidates` searches IGDB (when configured) and
the enabled community providers in priority order. Steam is the fallback: it
is queried when no other provider is configured, or when the request asks for
`provider=steam` explicitly. Responses are ranked by a lenient title/year score
so a weak automatic match is still pickable:

```ts
interface MetadataCandidate {
  provider: string; providerName: string; providerId: string;
  providerUrl: string | null; title: string; year: number | null;
  coverUrl: string | null; summary: string | null;
  developers: string[]; platforms: string[];
  score: number; confidence: number; // 0-1
  selected: boolean;                // matches the game's current provider id
}
interface MetadataProviderStatus {
  slug: string; name: string; ok: boolean;
  error: string | null; candidates: number; elapsedMs: number;
}
```

A provider that fails (timeout, unreachable, bad payload) appears in
`providers` with `ok: false`; the request still answers 200 with the other
providers' candidates. Unknown slugs and empty queries are `400
invalid_provider` / `400 invalid_query`.

`POST /api/games/{id}/metadata/apply` fetches the record by id (`IGDB
games/{id}`, Steam `appdetails?appids=`, community `GET {base_url}/games/{id}`)
and writes it to the game row. `provider.slug`, `provider.id`, `provider.url`
and the cached artwork always follow the selected record. Fields absent from
the record keep their current value. Metadata fields edited by hand through
`PATCH /api/games/{id}` are tracked in `games.manual_fields_json`
(`title`, `releaseDate`, `description`, `genres`, `developers`, `publishers`,
`rating`, `ageRating`, `altTitles`, `players`) and are preserved unless the
request sets `replaceManual: true`; the candidates response lists them as
`manualFields`. A missing record is `404 metadata_candidate_not_found`, a
provider failure `502 provider_request_failed`. Replaced artwork files become
unreferenced and are collected by the media GC after its grace period.

### Version fingerprints

`archiveHash` is a **sampled** fingerprint, not a full content hash: SHA-256 over
the file size, the first MiB and the last MiB (`sha256-sample:<hex>`, see
`ludex_core::fingerprint`). It changes when an archive is replaced, costs two
read windows per file, and is computed by the scanner (new files immediately,
older rows over the next scans). `updatedAt` moves whenever the row is
re-indexed. Clients compare `versionId` first and fall back to the fingerprint
when a version was re-uploaded in place.

### Library filters and search

`yearFrom`/`yearTo` are inclusive release years. `players` is the minimum
supported player count (`maxPlayers`). `playtimeMin`/`playtimeMax` are inclusive
minutes played by the requesting user. `minRating` is 0-100. `provider` is a
metadata slug (`igdb`, `steam`). All filters combine with `AND` and are safe to
share in a URL.

When `q` is present, titles, sort titles and provider aliases are matched
server-side with diacritic-insensitive fuzzy scoring (trigram + Levenshtein
fallback). Results are ranked by relevance and carry `matchedField`,
`matchedTitle` and `matchScore`; `sort` is ignored in that mode. `GET
/api/games/suggest` exposes the same scorer for type-ahead suggestions.

### Progress / bookmarks

| Method | Route | Body | Response |
|---|---|---|---|
| PUT | `/api/games/{id}/progress` | `{ state }` | `{ state, minutesPlayed, lastPlayedAt }` |
| POST | `/api/games/{id}/playtime` | `{ minutes }` | updated progress |
| PUT | `/api/games/{id}/bookmark` | `{ bookmarked }` | `{ bookmarked }` |
| POST | `/api/games/{id}/installed` | desktop install report → `{ recorded }` |
| GET | `/api/stats` | — | `{ games, versions, totalBytes, users, byState: Record<ProgressState, number> }` |

## Share links (#9)

Editor-managed, account-free links to one game or one version. The plaintext
token is returned **once**, at creation; only its SHA-256 is stored.

| Method | Route | Auth | Notes |
|---|---|---|---|
| POST | `/api/games/{id}/shares` | editor | body `{ versionId?, expiresInHours?, password?, maxDownloads?, label? }` → 201 `ShareLink` (includes `token`); 404 `game_not_found`; 400 `invalid_expiry` (`1..=8760`), `invalid_max_downloads` (`1..=100000`), `invalid_password` (min 4 chars), `invalid_label`, `invalid_version` (version not under this game) |
| GET | `/api/shares` | editor | every link, newest first, with `gameTitle`; never includes `token` |
| DELETE | `/api/shares/{id}` | editor | 204; 404 `share_not_found`; idempotent for an already revoked row |
| GET | `/api/share/{token}` | public | `ShareView` (metadata only); 404 `share_not_found` for unknown, revoked, expired or exhausted links |
| POST | `/api/share/{token}/access` | public | body `{ password? }` (omitted body = no password) → `{ url, expiresAt }`; 401 `invalid_share_password`, 410 `share_expired`, 410 `share_exhausted`, 404 `share_not_found`, 429 `rate_limited` |

The web client serves the public page at `#/share/{token}` outside its auth
gate, and editors create, list and revoke links from the game detail page.

```ts
interface ShareLink {
  id: number; token?: string;          // token only in the creation response
  gameId: number; versionId: number | null; // null = latest at access time
  gameTitle: string | null;            // management list only
  label: string | null;
  expiresAt: string | null;            // RFC 3339
  hasPassword: boolean;
  maxDownloads: number | null;         // null = unlimited
  downloadCount: number;
  revokedAt: string | null; createdAt: string;
}

interface ShareView {
  game: {
    id: number; title: string; description: string | null;
    coverUrl: string | null; backgroundUrl: string | null; screenshots: string[];
    releaseDate: string | null; rating: number | null;
    genres: string[]; developers: string[]; publishers: string[];
    ageRating: number | null;
  };
  version: {
    id: number; version: string | null; size: number;
    releaseDate: string | null; earlyAccess: boolean;
  };
  expiresAt: string | null; hasPassword: boolean;
  remainingDownloads: number | null;   // null = unlimited
}
```

Security model:

- **No library exposure.** The public page is built from a dedicated `games`
  query: no `progress`, `bookmarks`, `playSessions`, file paths, versions list
  or other games. `ShareView` is metadata plus a short-lived download ticket.
- **Expiry.** Optional `expiresAt`; the access route answers 410 `share_expired`
  and the metadata page answers 404. `versionId: null` resolves the newest
  available version at access time, so a link survives a library update.
- **Password.** Optional, min 4 characters, stored as Argon2id. Wrong or missing
  password → 401 `invalid_share_password`; the token itself is still subject to
  the rate limit. Every attempt performs exactly one Argon2 verification, so
  latency does not reveal whether a token exists or is protected.
- **Download cap.** `maxDownloads` is reserved atomically (`UPDATE … WHERE
  download_count < max_downloads`), so parallel requests cannot exceed it. The
  cap is reached → 410 `share_exhausted`.
- **Revocation.** `DELETE /api/shares/{id}` sets `revoked_at`; every public
  route answers 404 from then on. Revoking an unknown link is a 404.
- **Rate limiting.** 30 `POST /api/share/{token}/access` attempts per token per
  sliding 60 s window → 429 `rate_limited`. Attempts are consumed even when the
  password is correct.
- **Tickets, not links to the file.** `POST …/access` returns
  `/api/games/{id}/download?versionId={id}&token=…`, a 15-minute JWT scoped to
  the game and version. Range requests, `Content-Disposition` and object
  storage work exactly like the authenticated download.
- **Audit.** `share.created`, `share.revoked`, `share.access` and
  `share.access_denied` are recorded; the token and password never appear in
  logs, audit metadata or responses.

## Collections (#25)

Collections are per-user shelves. `kind=manual` stores an explicit list;
`kind=smart` stores a rule tree and evaluates it at read time. Every account can
read public collections; private collections return 404 to anyone but the owner.

```ts
type CollectionKind = "manual" | "smart";
type SmartField =
  | "genre" | "year" | "players" | "playtime"
  | "rating" | "installed" | "provider" | "title";

interface SmartRule {
  field: SmartField;
  /** Defaults: "is" except title → "contains", players/playtime/rating → "gte". */
  op?: "is" | "contains" | "gte" | "lte" | "between";
  value: string | number | boolean;
  value2?: string | number; // upper bound for "between"
}
interface SmartRules { match: "all" | "any"; rules: SmartRule[] }

interface Collection {
  id: number; name: string; description: string | null;
  isPublic: boolean; kind: CollectionKind; rules: SmartRules;
  gameCount: number; owner: { id: number; username: string; displayName: string | null };
  owned: boolean; createdAt: string; updatedAt: string;
}
```

Rules combine with `all` (AND) or `any` (OR). An empty rule tree matches
nothing, so a fresh smart collection reads as empty. `installed` means "a
downloadable version exists" because the server cannot see the desktop's local
installs. Playtime and installed state are evaluated for the requesting user.
Unknown fields, operators or value types return `400 invalid_rules` on write.

| Method | Route | Notes |
|---|---|---|
| GET | `/api/collections` | own collections plus public ones, own first |
| POST | `/api/collections` | `{ name, description?, isPublic?, kind?, rules?, gameIds? }` → 201 `Collection` |
| GET | `/api/collections/{id}` | owner or public, else 404 |
| PATCH | `/api/collections/{id}` | owner; partial fields; switching manual→smart clears the explicit list |
| DELETE | `/api/collections/{id}` | owner; 204 |
| GET | `/api/collections/{id}/games` | `?page=&limit=` → `Paginated<GameSummary>`; smart rules evaluated live |
| POST | `/api/collections/{id}/games` | owner, manual only; `{ gameIds: number[] }` → `{ added, total }`; smart → 409 |
| DELETE | `/api/collections/{id}/games/{gameId}` | owner, manual only; 204 |

Names are unique per owner (case-insensitive) → `409 collection_exists`.

## Community (#10)

Profiles are opt-in. `users.public_profile` defaults to false; only opted-in
accounts appear in the directory, the feed and profile pages. Private profiles
return 404 to everyone else, including their activity.

```ts
interface PublicProfile {
  id: number; username: string; displayName: string | null; bio: string | null;
  avatarUrl: string | null; publicProfile: boolean; createdAt: string;
}
interface PublicStats {
  games: number; completed: number; minutesPlayed: number;
  bookmarked: number; collections: number;
}
interface PublicProfileResponse {
  user: PublicProfile; stats: PublicStats;
  library: GameSummary[];   // progress or bookmark rows, most recent first
  collections: Collection[]; // public collections (all of them for the owner)
}
type ActivityKind =
  | "game_added" | "game_installed" | "game_played"
  | "game_completed" | "collection_created" | "achievement_unlocked";
interface Activity {
  id: number; kind: ActivityKind; createdAt: string;
  metadata: Record<string, unknown>;
  user: { id: number; username: string; displayName: string | null; avatarUrl: string | null };
  game: { id: number; title: string; coverUrl: string | null; releaseYear: number | null } | null;
  collection: { id: number; name: string } | null;
}
interface ActivityPage { items: Activity[]; nextBefore: number | null }
```

| Method | Route | Notes |
|---|---|---|
| GET | `/api/me/profile` | own profile, including `publicProfile` |
| PATCH | `/api/me/profile` | `{ publicProfile?, displayName?, bio? }`; `null` clears text |
| GET | `/api/community/users` | public profiles, alphabetical |
| GET | `/api/community/users/{id}` | `PublicProfileResponse`; private → 404 for others |
| GET | `/api/community/users/{id}/activity` | `?limit=&before=` → `ActivityPage` |
| GET | `/api/community/feed` | public activity, newest first; `?limit=&before=` |

Activity is recorded server-side: `game_added` when a game first enters the
account's library (first progress row or playtime report), `game_played` on a
playing state or playtime report (deduped 30 min), `game_completed` on the
completed state (deduped 60 min), `collection_created` on creation, and
`game_installed` from `POST /api/games/{id}/installed` (deduped 60 min; the
desktop calls it). Feed entries survive deletion of the referenced game or
collection as `null` references.

## Cloud saves (per user)

Per-user save snapshots, manual or automatic (#108). The desktop archives the
save paths detected by #107 and uploads them after each session; the server
stores opaque archives and never inspects them.

```ts
interface SaveSnapshot {
  id: number; gameId: number; versionId: number | null;
  label: string; fileName: string; size: number; sha256: string;
  createdAt: string;
  /** `manual` for a user upload, `auto` for the post-session sync (#108). */
  source: "manual" | "auto";
  /** Opaque device identifier; display only, never authorisation. */
  deviceId: string | null;
}
```

| Method | Route | Notes |
|---|---|---|
| GET | `/api/games/{id}/saves` | current user's snapshots, newest first |
| POST | `/api/games/{id}/saves` | multipart `file`, optional `label`, `source` (`manual` \| `auto`), optional `deviceId`; max 2 GiB. An `auto` upload whose SHA-256 already exists for the user and game returns `200` with the existing snapshot; a new one returns `201` |
| GET | `/api/saves/{id}/download` | owner (or admin) download, `Content-Disposition` |
| DELETE | `/api/saves/{id}` | owner (or admin), 204 |

Storage: `{data_dir}/saves/{user_id}/{game_id}/{snapshot_id}-{sanitized_name}`.
A user can never see another user's snapshots. Automatic uploads are pruned to
the newest **10 per user and game**; manual snapshots are never pruned. The
desktop game page shows the latest snapshot, last upload/restore, effective
automatic-backup toggle and a conflict dialog; `lib/cloud-saves.ts` wraps the
Tauri commands. The browser build can still upload/download manual archives and
states that automatic sync needs the desktop app.

## Users (admin)

| Method | Route | Notes |
|---|---|---|
| GET | `/api/users` | `User[]`; never includes password hashes or OIDC subjects |
| POST | `/api/users` | `{ username, password, email?, role }` → `User` (always `source: "local"`) |
| PATCH | `/api/users/{id}` | `{ username?, email?, password?, role?, activated? }` → `User` |
| DELETE | `/api/users/{id}` | 204; cannot delete the last admin or self |

`PATCH` fields:

- `username` — local accounts only; validated and unique (`409 username_taken`).
- `email` — local accounts only; `null` or `""` clears it.
- `password` — local accounts only, write-only; at least 8 characters
  (`400 invalid_password`). Hashed with Argon2 and never returned, logged or
  copied into audit metadata. A reset records `user.password_reset`.
- `role`, `activated` — always editable, including for SSO-linked accounts:
  they are local authorisation controls, not identity data.
- Onboarding fields (`onboardingCompletedAt`, `onboardingStep`) are deliberately
  **not** writable here; only the account itself can change them through
  `PUT /api/auth/onboarding`.

Identity is owned by the OIDC provider once an account is linked: attempts to
change `username`, `email` or `password` fail with `409 managed_by_idp` and a
message naming the issuer. Unlinking or re-linking an identity is out of scope.
`user.updated` metadata only lists the changed field names in `fields` (no
values, passwords, hashes or subjects).

Existing guards stay in force: an administrator cannot deactivate or delete
themselves while active, and the last active administrator cannot be demoted,
deactivated or deleted (`409 last_admin`, `422 cannot_deactivate_self`).
Changing your own username or password through this route is allowed.

## Admin OIDC settings (#103)

Administrators configure SSO from **Admin → Authentication**. All routes
require an administrator session (`403 forbidden` otherwise). The client secret
is write-only: no response ever includes it, and audit metadata never contains
it. See `docs/OIDC.md` for precedence, encryption and rotation details.

| Method | Route | Notes |
|---|---|---|
| GET | `/api/admin/oidc` | effective non-secret settings + `secretConfigured`, `envLockedFields`, `status`, `linkedAccounts` |
| PUT | `/api/admin/oidc` | validate the payload, encrypt a replacement secret, run discovery, persist and hot-swap; a failure leaves the previous config active |
| POST | `/api/admin/oidc/test` | discovery against the submitted values; no persistence |
| POST | `/api/admin/oidc/reset` | delete the saved row and fall back to `LUDEX_OIDC_*` only |

```ts
interface OidcSettings {
  enabled: boolean;
  issuer: string | null;
  clientId: string | null;
  redirectUrl: string | null;     // relative path or absolute URI
  publicBaseUrl: string | null;
  scopes: string;                 // space-separated, openid always added
  displayName: string;            // "Sign in with …" label
  autoCreate: boolean;
  defaultRole: "admin" | "editor" | "user";
  groupsClaim: string;            // dot path allowed
  adminGroups: string[];
  secretConfigured: boolean;      // true when a secret is stored at rest
}

interface OidcAdminResponse {
  settings: OidcSettings;
  envLockedFields: string[];      // camelCase names pinned by LUDEX_OIDC_*
  status: {
    ok: boolean;
    issuer: string | null;
    checkedAt: string | null;     // null = not checked since start
    error: string | null;
  };
  linkedAccounts: number;         // users with oidc_issuer set
}

interface OidcSettingsInput {
  enabled?: boolean;
  issuer?: string | null;
  clientId?: string | null;
  redirectUrl?: string | null;
  publicBaseUrl?: string | null;
  scopes?: string;
  displayName?: string;
  autoCreate?: boolean;
  defaultRole?: "admin" | "editor" | "user";
  groupsClaim?: string;
  adminGroups?: string[];
  replaceSecret?: string | null;  // write-only; blank keeps the stored secret
  clearSecret?: boolean;          // remove the secret (public PKCE client)
}
```

`PUT` returns the same `OidcAdminResponse` as `GET`. Validation failures use
`400 invalid_oidc_*` (`invalid_oidc_issuer`, `insecure_oidc_issuer`,
`invalid_oidc_client_id`, `invalid_oidc_redirect_url`,
`invalid_oidc_public_base_url`, `invalid_oidc_groups_claim`). A discovery
failure returns `502 oidc_discovery_failed` with the provider error in
`message`, and nothing is persisted or swapped. Requesting a save while the
stored secret cannot be decrypted (server secret changed) returns
`400 oidc_secret_unreadable` unless the request replaces or clears it.

`POST /api/admin/oidc/test` returns:

```ts
interface OidcTestResult {
  ok: true;
  discovery: {
    issuer: string;
    authorizationEndpoint: string;
    tokenEndpoint: string;
    jwksUri: string;
    idTokenSigningAlgValuesSupported: string[];
    tokenEndpointAuthMethodsSupported: string[];
  };
}
```

Each successful save or test records `settings.oidc_updated` in the audit log
with `{ issuer, enabled, fields }` (field names only) or `{ reset: true, … }`
for a reset.

## API keys (#40)

API keys are random `ldx_<64 hex>` bearer tokens. The plaintext token is
returned exactly once by `POST /api/keys`; only its SHA-256 and a non-secret
`prefix` are stored. Keys inherit their owner's role but are limited by their
scopes: `read` (GET), `write` (mutations: progress, saves, library scan,
ingest, key management) and `admin` (administrator routes). `admin` implies
`write`, which implies `read`.

```ts
interface ApiKey {
  id: number; userId: number; name: string; prefix: string;
  scopes: ("read" | "write" | "admin")[];
  createdAt: string; lastUsedAt: string | null; revokedAt: string | null;
}
// POST response adds `secret: string` (never returned again).
```

| Method | Route | Notes |
|---|---|---|
| GET | `/api/keys` | own keys, newest first; `?all=true` (admin) lists every account |
| POST | `/api/keys` | `{ name, scopes?, userId? }` → 201 `ApiKey & { secret }`; `userId` requires admin |
| DELETE | `/api/keys/{id}` | revoke (idempotent, 204); owner or admin |

`Authorization: Bearer ldx_...` is accepted everywhere a JWT is. Every use
updates `lastUsedAt`. Revoked or unknown keys get the same 401 as a bad JWT.

## Audit log (#42)

Every security-relevant event is written to `audit_logs` with an actor
(`user`, `api_key` or `system`), action, optional target, secret-free JSON
metadata, IP when known, and timestamp. Recorded actions: `login.succeeded`,
`login.failed`, `user.created|updated|deleted|password_changed|password_reset`,
`api_key.created|revoked`,
`game.ingested|deleted`, `game.metadata_matched`, `download.completed`, `settings.updated`,
`settings.oidc_updated`, `settings.metadata_provider_updated`,
`webhook.created|updated|deleted`,
`save.uploaded|deleted`, `oidc.login`, `oidc.linked`,
`oidc.handoff_created|claimed|failed`, `diagnostics.deleted`.

```ts
interface AuditLog {
  id: number; actorId: number | null; actorKind: "user" | "api_key" | "system";
  action: string; targetType: string | null; targetId: string | null;
  metadata: Record<string, unknown>; ip: string | null; createdAt: string;
}
interface AuditPage { items: AuditLog[]; nextBefore: number | null }
```

| Method | Route | Notes |
|---|---|---|
| GET | `/api/audit` | admin + `admin` scope. Query: `limit` (50, max 200), `before` (id cursor), `actorId`, `actorKind`, `action`, `targetType`, `targetId` |

Newest first. Pass `nextBefore` as `before` to fetch the next page; `null`
means the last page.

## Anonymous diagnostics (#171)

Opt-in crash and error reports. The toggle is off by default and lives on the
device (`LocalSettings.diagnosticsEnabled`); the server only sees reports a user
explicitly sent after previewing the exact payload. A report is not linked to an
account: the row carries a random install id generated on the device, never a
username, user id or IP address. The server keeps reports for 90 days
(`DIAGNOSTICS_RETENTION_DAYS`) and sweeps expired rows on every insert.

```ts
interface DiagnosticsReportInput {
  installId: string;    // [A-Za-z0-9_-]{8,64}, random, generated locally
  appVersion: string;   // 1-64 chars
  platform: "linux" | "windows" | "macos" | "web" | "android" | "ios" | "other";
  os: string;           // 1-120 chars
  locale?: string;      // up to 32 chars
  channel?: string;     // up to 32 chars
  errorKind: "panic" | "startup" | "command" | "api" | "install" | "launch" | "update" | "web" | "other";
  summary: string;      // 1-2000 chars, redacted again server-side
  detail?: Record<string, unknown>; // object, serialised cap 16 KiB, depth 4
}

interface DiagnosticsReport { id: number; installId: string; appVersion: string; platform: string; os: string; locale: string; channel: string; errorKind: string; summary: string; detail: Record<string, unknown>; createdAt: string }
interface DiagnosticsPage {
  items: DiagnosticsReport[]; nextBefore: number | null; total: number;
  byVersion: { value: string; count: number }[];
  byKind: { value: string; count: number }[];
  byPlatform: { value: string; count: number }[];
  retentionDays: number;
}
```

| Method | Route | Notes |
|---|---|---|
| POST | `/api/diagnostics` | public; body above, capped at 24 KiB (larger bodies get `413 report_too_large`). Throttled per address (`LUDEX_DIAGNOSTICS_RATE_LIMIT`, default 10 per hour, 0 disables; `429 rate_limited`). Unknown fields are rejected with `400 invalid_report`; field errors use `invalid_install_id`, `invalid_platform`, `invalid_error_kind`, `invalid_summary`, `invalid_detail`, `detail_too_large`. Returns `202 { accepted: true }` |
| GET | `/api/diagnostics` | admin + `admin` scope. Query: `limit` (50, max 200), `before` (id cursor), `kind`. Returns `DiagnosticsPage`; `null` `nextBefore` is the last page |
| DELETE | `/api/diagnostics/{id}` | admin + `admin` scope; 204 on success, 404 `report_not_found`. Emits `diagnostics.deleted` in the audit log |

Every string is run through `ludex_core::redact` before storage (client-side
first, then again on the server): URLs → `<url>`, absolute paths →
`<path:hash>`, IPv4/IPv6 → `<ip>`, e-mails → `<email>`, `Bearer` credentials and
`token=…` values → `<token>`, plus the configured home directory, username and
hostname. Reports are read-only for everyone except the admin delete route.

## Webhooks (#41)

Administrators register outbound HTTP endpoints. Deliveries are asynchronous:
each event is stored in `webhook_deliveries`, signed with HMAC-SHA256 over the
exact JSON body, retried up to 5 times with exponential backoff (1 s, 2 s, 4 s,
8 s; base configurable with `LUDEX_WEBHOOKS_BACKOFF_MS`) and a 10 s request
timeout. Non-2xx responses and network errors are retried; redirects are not
followed. Loopback/private/link-local targets are rejected unless
`LUDEX_WEBHOOKS_ALLOW_PRIVATE=true` (tests/LAN only).

Events: `game.ingested`, `game.deleted`, `download.completed`, `user.created`,
`save.uploaded`. The test endpoint queues a `webhook.test` delivery.

Request headers:

```
Content-Type: application/json
X-Ludex-Signature: sha256=<hex HMAC-SHA256 of the raw body with the webhook secret>
X-Ludex-Event: game.ingested
X-Ludex-Delivery: 42
```

Payload:

```json
{
  "event": "game.ingested",
  "timestamp": "2026-11-10T12:00:00Z",
  "data": { "title": "Celeste", "source": "scan" }
}
```

```ts
interface Webhook {
  id: number; url: string; events: string[]; enabled: boolean; createdAt: string;
}
// POST response adds `secret: string`; PATCH adds it only when rotateSecret=true.
```

| Method | Route | Notes |
|---|---|---|
| GET | `/api/webhooks` | admin + `admin` scope; secrets never returned |
| POST | `/api/webhooks` | `{ url, events, enabled? }` → 201 `Webhook & { secret }` |
| PATCH | `/api/webhooks/{id}` | `{ url?, events?, enabled?, rotateSecret? }` → `Webhook` (+ new `secret` if rotated) |
| DELETE | `/api/webhooks/{id}` | 204 |
| POST | `/api/webhooks/{id}/test` | 202 `{ deliveryId }`; webhook must be enabled |

## Metadata providers (#28)

Administrators install community metadata sources as HTTP sidecars. No
third-party code runs inside the server. IGDB (when configured) is always tried
first; enabled providers run next in ascending `priority`, then the Steam Store
fallback, then the file-name parser. A provider only fills metadata that is
still empty and a failing provider never fails an enrichment or a scan.

Provider contract: `GET {baseUrl}/search?title=<urlencoded>&year=<optional>`
with `Accept: application/json` and, when configured, `Authorization: Bearer
<token>`. The response is `{ "results": [ProviderCandidate, ...] }`; at most 50
entries are read. Optional `GET {baseUrl}/games/{id}` returns one candidate or a
`results` envelope.

```ts
interface ProviderCandidate {
  id: string; title: string;
  description?: string; releaseDate?: string; coverUrl?: string;
  backgroundUrl?: string; screenshots?: string[]; genres?: string[];
  developers?: string[]; publishers?: string[]; tags?: string[];
  rating?: number; ageRating?: number; providerUrl?: string;
}

/** Canonical fields a mapping may target. */
type MetadataProviderField =
  | "id" | "title" | "description" | "releaseDate" | "coverUrl" | "backgroundUrl"
  | "screenshots" | "genres" | "developers" | "publishers" | "tags" | "rating"
  | "ageRating" | "providerUrl";

interface MetadataProvider {
  id: number; name: string; baseUrl: string;
  /** Whether a bearer token is stored. The token is never returned. */
  secretConfigured: boolean;
  priority: number; enabled: boolean;
  /** Canonical field -> dot path in a non-canonical response. */
  mapping: Partial<Record<MetadataProviderField, string>>;
  createdAt: string; updatedAt: string;
}

interface ProviderTestResult {
  ok: boolean; candidates: ProviderCandidate[];
  error?: string; statusCode?: number; elapsedMs: number;
}
```

| Method | Route | Notes |
|---|---|---|
| GET | `/api/admin/metadata-providers` | admin + `admin` scope; ordered by priority |
| POST | `/api/admin/metadata-providers` | `{ name, baseUrl, token?, priority?, enabled?, mapping? }` → 201 `MetadataProvider`; new providers default to disabled |
| PUT | `/api/admin/metadata-providers/{id}` | `{ name?, baseUrl?, token?, clearToken?, priority?, enabled?, mapping? }` → `MetadataProvider`; blank/absent `token` keeps the stored one |
| DELETE | `/api/admin/metadata-providers/{id}` | 204; fetch history is cascaded |
| POST | `/api/admin/metadata-providers/{id}/test` | `{ title, year? }` → 200 `ProviderTestResult`; provider failures are reported in the body, not as 5xx |
| POST | `/api/admin/metadata-providers/reorder` | `{ ids: number[] }` → `MetadataProvider[]`; priorities become 10, 20, ... |

Requests are server-side only, time out after 10 s, cap the body at 2 MiB and
disable redirects. Private/loopback targets are refused unless
`LUDEX_PROVIDER_ALLOW_PRIVATE=true` (tests only). Tokens are encrypted at rest
(AES-256-GCM, HKDF-derived key, purpose-bound AAD) and are never serialised.
Every attempt is recorded in `provider_fetches` (provider, game, ok, status,
short error) with a 30-day retention. Writes emit
`settings.metadata_provider_updated` with the id, name, enabled, priority and
changed field names, never the token. Field mapping validation accepts only the
canonical fields above and dot-separated object keys.

## Library

| Method | Route | Notes |
|---|---|---|
| POST | `/api/library/scan` | admin/editor; 202 `{ started: boolean }`; scans every enabled root |
| GET | `/api/library/scan` | `ScanStatus` |
| GET | `/api/library/folders` | `{ gamesDir, mediaDir, dataDir }` (admin) |
| GET | `/api/library/roots` | admin + `admin` scope; `LibraryRoot[]`, ordered |
| POST | `/api/library/roots` | admin; `{ path, label?, enabled?, kind? }` (`kind`: `game` default or `rom`) → 201 `LibraryRoot`; 409 `root_exists` |
| PATCH | `/api/library/roots/{id}` | admin; `{ label?, path?, enabled?, position?, kind? }` → `LibraryRoot`; 409 `root_kind_locked` when indexed versions exist |
| DELETE | `/api/library/roots/{id}?force=` | admin; 204; 409 `root_not_empty` unless `force=true` |
| POST | `/api/library/roots/{id}/scan` | admin/editor; 202 `{ started, rootId }`; scans that root only |
| GET | `/api/library/watcher` | admin; `LibraryWatcherStatus` |

```ts
interface LibraryRoot {
  id: number; label: string; path: string;
  enabled: boolean; available: boolean; position: number;
  createdAt: string; lastScanAt: string | null; lastError: string | null;
  versionCount: number;
  /** `game` for game archives, `rom` for an emulator ROM library (#52). */
  kind: "game" | "rom";
}

interface LibraryWatcherStatus {
  enabled: boolean; debounceMs: number;
  roots: { id: number; label: string; path: string }[];
  lastEventAt: string | null; lastScanAt: string | null;
  pending: boolean; lastError: string | null;
}
```

A root whose path is missing or unmounted is reported with `available: false`
and a `lastError`; its versions keep `missing = 0` and downloads answer
`404 root_unavailable`. Removing a root keeps games, progress and cloud saves.
See [`LIBRARY.md`](./LIBRARY.md) for scan ordering, mirror deduplication and
watcher platform limits.

## Integrity checks

| Method | Route | Notes |
|---|---|---|
| POST | `/api/integrity/check` | admin + `admin` scope; `{ scope?: "all" \| "archives" \| "media" }` → 202 `{ reportId, status }`; 409 `integrity_running` |
| GET | `/api/integrity/reports?limit=` | admin; newest first, `limit` 1–200 (default 20) |
| GET | `/api/integrity/reports/{id}` | admin; `{ report, findings }`; 404 `report_not_found` |

```ts
type IntegrityScope = "all" | "archives" | "media";

interface IntegrityReport {
  id: number; scope: IntegrityScope;
  status: "running" | "completed" | "failed";
  checked: number; findingsCount: number;
  error: string | null; startedAt: string; finishedAt: string | null;
}

interface IntegrityFinding {
  id: number; kind: "missing" | "corrupt" | "size_mismatch";
  targetType: "archive" | "media"; targetId: string | null;
  path: string | null; detail: string | null; createdAt: string;
}
```

Checks never delete anything: findings are informational. The first successful
hash of an archive or media file becomes its baseline. Scheduled by
`LUDEX_INTEGRITY_INTERVAL_HOURS` (default 168, `0` disables).

## Object storage (#47)

Game archives can live on local roots (`local`, default) or in an
S3-compatible bucket (`s3`). Metadata, media and saves stay on local disk. The
API shape is unchanged: downloads and stream installs are proxied through the
server with the same auth rules.

`game_versions` gains `storage_backend` (`local`/`s3`), `object_key` and `etag`
(migration `0010_storage_ops.sql`). These columns are not exposed in
`GameVersion`; the download URL is identical for both backends.

| Behaviour | Result |
|---|---|
| `GET /api/games/{id}/download` on an `s3` row | proxied object, single `Range` supported (`206`/`416`), `Content-Disposition`, no redirect |
| object missing from the bucket | local file if present, else `404 file_not_found` |
| bucket unreachable / 5xx | local file if present, else `502 storage_unavailable` |
| `s3` backend configured but no usable client | local file if present, else `503 storage_unavailable` |
| `local` row (including all pre-#47 rows) | served from the library root as before |
| `GET …/local-path` on an `s3` row | `404 local_path_unavailable` |
| `GET …/stream`, `…/stream/manifest`, `…/stream/entries/{index}` on an `s3` row | archive staged under `{DATA}/tmp`, then streamed/served; the staged copy is removed when the response ends |

Configuration and operations: [`STORAGE.md`](./STORAGE.md).

## Metrics and health details (#48)

`GET /metrics` (root path, not under `/api`) exposes Prometheus text format:

- `ludex_http_requests_total{method,route,status}` and
  `ludex_http_request_duration_seconds{method,route}` with fixed buckets;
- `ludex_games`, `ludex_game_versions{missing}`, `ludex_library_roots{available}`;
- `ludex_ingest_jobs{status}`, `ludex_scan_running`,
  `ludex_scan_last_success_timestamp_seconds`, `ludex_scan_duration_seconds`,
  `ludex_scan_files{result}`;
- `ludex_webhook_deliveries{state}`, `ludex_webhook_retries`;
- `ludex_media_cache_files`, `ludex_media_cache_bytes`,
  `ludex_disk_free_bytes{volume}`, `ludex_database_size_bytes`;
- `ludex_process_uptime_seconds`, `ludex_build_info{version,commit}`,
  `ludex_storage_backend_info{backend}`, `ludex_integrity_findings{status}`.

When `LUDEX_METRICS_TOKEN` is set, scrapes require
`Authorization: Bearer <token>` (`401` otherwise). Without it, `/metrics` is
public but limited to 30 scrapes per minute per client IP (`429` beyond that).
Route labels use the axum route template (`/api/games/{id}`), never the concrete
id, so cardinality stays bounded.

Prometheus scrape configuration:

```yaml
scrape_configs:
  - job_name: ludex
    scrape_interval: 30s
    authorization:
      type: Bearer
      credentials_file: /etc/prometheus/ludex-metrics-token
    static_configs:
      - targets: ["ludex.lan:8080"]
```

## Media cache garbage collection

| Method | Route | Notes |
|---|---|---|
| POST | `/api/media/gc?dryRun=true\|false` | admin + `admin` scope; `dryRun` defaults to `true` |

```ts
interface MediaGcResult {
  dryRun: boolean; graceHours: number;
  scanned: number; referenced: number;
  orphanFiles: number; orphanBytes: number;
  deletedFiles: number; deletedBytes: number;
  candidates: string[]; // up to 100 orphan file names
  startedAt: string; finishedAt: string;
}
```

Only unreferenced files older than `LUDEX_MEDIA_GC_GRACE_HOURS` (default 24)
are removed; files referenced by a game or user row are never deleted. A
scheduled run is enabled by `LUDEX_MEDIA_GC_INTERVAL_HOURS` (default `0`).

## Ingest (admin/editor) — adapted from gv-downloader

| Method | Route | Notes |
|---|---|---|
| GET | `/api/ingest/downloads` | `IngestJob[]`, newest first |
| POST | `/api/ingest/downloads` | `{ url, title? }` → 201 `IngestJob` |
| GET | `/api/ingest/downloads/{id}` | `IngestJob` |
| DELETE | `/api/ingest/downloads/{id}` | cancel; deletes `.part`; 204 |
| POST | `/api/ingest/downloads/{id}/retry` | resume error/cancelled job |
| PUT | `/api/ingest/downloads/{id}` | `{ url }` replace URL, requeue if stopped |
| GET | `/api/ingest/files` | `{ name, size, partial, modifiedAt }[]` |
| DELETE | `/api/ingest/files/{name}` | delete a library file; 204 |
| POST | `/api/ingest/files/{name}/verify` | full archive test → `{ ok, log }` |
| GET | `/api/ingest/stats` | `{ gamesDir, diskTotal, diskFree, libraryBytes, gameCount, activeJobs }` |

A successful ingest triggers `library/scan` automatically.

## News

| Method | Route | Notes |
|---|---|---|
| GET | `/api/news` | `NewsPost[]` |
| POST | `/api/news` | admin; `{ title, body }` |
| PATCH | `/api/news/{id}` | admin |
| DELETE | `/api/news/{id}` | admin |

## Server config

| Method | Route | Notes |
|---|---|---|
| GET | `/api/config` | public; `{ serverName, version, features: { igdb, steam, ingest, localMode }, maxUploadBytes }` |

## Desktop (Tauri) commands

The web bundle runs in both browsers and Tauri. `apps/web/src/lib/native.ts`
detects Tauri (`window.__TAURI_INTERNALS__`) and calls these commands; in a
browser every function reports `available: false`.

```ts
interface LocalSettings {
  mode: "local" | "remote";
  serverUrl: string;            // remote URL, or the embedded server URL
  token: string | null;         // JWT
  gamesDir: string | null;      // local library folder
  installDir: string;           // mirrors the default install location (#99)
  installRootConfirmed: boolean; // user confirmed a drive-based install root (#145)
  downloadDir: string;          // mirrors the default download location; empty = {installDir}/.ludex-tmp
  locations: InstallLocation[]; // named install/download/temp folders (#99)
  serverPort: number;           // preferred embedded port, 0 = random
  streamInstall: boolean;       // remote ZIP/7z/RAR installs without storing the archive
  keepArchives: boolean;        // keep the downloaded archive after install
  archivePasswords: string[];   // tried in order
  bandwidthLimitKib: number;    // global download cap in KiB/s, 0 = unlimited
  closeToTray: boolean;         // closing the window hides to the tray (default true)
  nativeTitleBar: boolean;      // OS title bar instead of Ludex's own (#111), default false
  updateChannel: "stable" | "early"; // app updater channel (default stable)
  checkUpdatesOnStartup: boolean;    // throttled to once per 24 h
  lastUpdateCheckAt: string | null;  // RFC 3339, drives the throttle
  skippedUpdateVersion: string | null; // #163: no prompt/badge for this version
  diagnosticsEnabled: boolean;       // #171: opt-in anonymous reports, default false
  diagnosticsInstallId: string;      // #171: random local id, cleared when disabled
  discordPresence: boolean;          // Discord Rich Presence (#12), default false
  discordClientId: string;           // Discord application id, not a bot token
  installerSilentArgs: string[][];   // silent install attempts (#7), empty = presets
  installerTimeoutSecs: number;      // per attempt, default 600, clamped 30-7200
  steamRoot: string;                 // Steam folder override (#13), empty = detect
  compatUmuPath: string;             // umu-run override (#90), empty = detect
  compatProtonPath: string;          // Proton folder/script override (#90), empty = detect
  compatWinePath: string;            // wine override (#90), empty = detect
  defaultLaunchMode: "auto" | "native" | "proton" | "umu"; // new games on Linux (#89)
  ludusaviManifestPath: string;      // explicit manifest file (#107), empty = managed copy then a detected install
  ludusaviBinary: string;            // ludusavi executable (#107), empty = PATH; fallback only
  ludusaviManifestAutoUpdate: boolean; // startup refresh (#153), default true, max once per 24 h
  ludusaviManifestUrl: string;       // mirror/pinned manifest URL (#153), empty = the canonical upstream manifest
  winePrefix: string;                // Wine/Proton prefix override (#107), empty = detect
  cloudAutoSync: boolean;            // global automatic save backup (#108), default true
  cloudDeviceId: string;             // random UUID generated on first use, not a hardware id
  cloudDeviceLabel: string | null;   // null = host name, then "Desktop"
  servers: ServerProfile[];          // saved servers (#24), metadata only
  activeServerId: string;            // profile backing serverUrl/token
  sunshineAppsPath: string;          // Sunshine apps.json override (#97), empty = detect
  sunshineConfPath: string;          // Sunshine sunshine.conf override (#97)
  moonlightBinary: string;           // Moonlight binary override (#98), empty = detect
  moonlightConfigPath: string;       // Moonlight config override (#98), empty = detect
  moonlightHost: string;             // manual host name/IP fallback (#151), empty = use saved hosts
  gamevaultRoots: string[];          // ordered GameVault install roots (#134)
  gamevaultScanDepth: number;        // executable search depth for scans, 4-8, default 6
  launcherSources: LauncherGame["source"][]; // sources launcher_scan may read (#155); all by default, empty = none
}

interface InstallLocation {
  id: string;                   // stable id; "" on create, filled by the backend
  label: string;                // 1-60 characters
  path: string;                 // absolute folder
  kind: "install" | "download" | "temp";
  default: boolean;             // at most one per kind
}

interface LocationStatus extends InstallLocation {
  exists: boolean;              // the folder itself exists right now
  writable: boolean;            // a probe file can be created
  freeBytes: number | null;     // null when the drive is missing
  totalBytes: number | null;
  note: string | null;          // missing drive, read-only, will be created…
  requiredBytes: number | null; // set by check_location
  freeSpaceOk: boolean | null;  // null when free space is unknown
}

/** #145: one detected install-root candidate; nothing is created. */
interface StorageSuggestion {
  path: string;                 // e.g. "C:\\Games", "~/Games"
  label: string;                // "C:", "Home", volume name
  volume: string;               // volume/mount root
  kind: "fixed" | "removable" | "network" | "other";
  exists: boolean;
  writable: boolean;
  freeBytes: number | null;
  totalBytes: number | null;
  recommended: boolean;
  current: boolean;             // equals the current default install root
  fallback: boolean;            // {appDataDir}/Games
  nearlyFull: boolean;          // < 10 GiB free or < 10% of the volume
  note: string | null;
}

/** #145. `needsChoice` is `!installRootConfirmed`; `install_game` accepts the
 *  path directly (`installDir`) while the root is not saved as a location. */
interface StorageSuggestions {
  suggestions: StorageSuggestion[];
  recommendedPath: string | null;
  currentPath: string;
  currentIsFallback: boolean;
  needsChoice: boolean;
  platform: "windows" | "macos" | "linux";
}

interface InstalledGame {
  gameId: number; versionId: number; title: string; version: string | null;
  installPath: string; executable: string | null; sizeBytes: number;
  installedAt: string; lastLaunchedAt: string | null;
  /** Fingerprint of the archive this install came from, when known. */
  archiveHash: string | null;
  /** `ludex` entries were installed here, `adopted` folders were linked in place (#129); everything else is a launcher import (#13, #116). */
  source: "ludex" | "adopted" | "steam" | "epic" | "gog" | "ea" | "ubisoft" | "xbox" | "itch" | "heroic" | "lutris";
  /** External launch URI (`steam://rungameid/<appid>`) for imported entries. */
  launchUri: string | null;
}

/** Preview of a silent installer run (#7, #93). */
interface InstallerPlan {
  executable: string | null;
  detected: string[];       // installer-looking files in the install folder
  attempts: string[][];     // argument sets, in order
  timeoutSecs: number;
  interactive: boolean;
  platform: string;         // "windows", "linux", "macos"
  compatTool: string | null; // Linux: tool a run would use (UMU/Proton/Wine)
  prefix: string | null;     // Wine prefix the installer would write into
}

interface InstallerAttempt {
  args: string[]; exitCode: number | null;
  timedOut: boolean; cancelled: boolean;
  error: string | null; durationMs: number;
}

interface InstallerReport {
  gameId: number; executable: string;
  success: boolean; cancelled: boolean; timedOut: boolean; interactive: boolean;
  exitCode: number | null; attempts: InstallerAttempt[];
  recordedExecutable: string | null; // set after a successful install
  candidates: string[];              // exe picker suggestions
  compatTool: string | null;         // Linux tool used (#93)
  logTail: string | null;            // last 8 KiB of the Linux installer log
  message: string;
}

/** Discord Rich Presence state (#12). */
interface PresenceStatus {
  enabled: boolean; configured: boolean; connected: boolean;
  lastError: string | null; details: string | null; retryInSeconds: number;
}

/** One installed Steam game discovered on disk (#13). */
interface SteamGame {
  appId: number; name: string; installDir: string; libraryPath: string;
  sizeBytes: number; lastPlayed: number | null;
  matchedGameId: number | null; matchedTitle: string | null;
  imported: boolean; localGameId: number; // negative app id when unmatched
}

interface SteamPreview {
  roots: string[]; games: SteamGame[]; warnings: string[];
  matched: boolean;  // false: the server could not be listed
}

interface SteamImportReport {
  imported: InstalledGame[];
  skipped: { appId: number; title: string; reason: string }

/** One game another launcher installed on this machine (#116). */
interface LauncherGame {
  source: "steam" | "epic" | "gog" | "ea" | "ubisoft" | "xbox" | "itch" | "heroic" | "lutris";
  externalId: string;               // Steam app id, Epic AppName, GOG product id…
  name: string;
  installDir: string | null;        // null when the launcher exposes no folder
  launchUri: string | null;         // null when the launcher has no launch id
  sizeBytes: number;
  lastPlayed: number | null;        // Unix seconds, when the launcher records it
  detail: string | null;            // launcher-side detail shown next to the title
  issue: string | null;             // why it needs a manual launch URI
  matchedGameId: number | null;
  matchedTitle: string | null;
  imported: boolean;
  localGameId: number;              // negative id used when nothing matched
}

interface LauncherSource {
  id: LauncherGame["source"];
  label: string;
  found: number;
  available: boolean;               // false: not installed / unsupported platform
  disabled: boolean;                // #155: turned off in Settings → Integrations; nothing was read
  detail: string;                   // path or registry key read, or why it was not
}

interface LauncherPreview {
  roots: string[];                  // files, folders and registry keys read
  games: LauncherGame[];
  sources: LauncherSource[];
  warnings: string[];
  matched: boolean;                 // false: the server could not be listed
}

interface LauncherImportEntry {
  source: LauncherGame["source"];
  externalId: string;
  gameId?: number | null;           // manual mapping, overrides title matching
  launchUri?: string | null;        // manual URI for launchers without a deep link
}

interface LauncherImportReport {
  imported: InstalledGame[];
  skipped: { source: string; externalId: string; title: string; reason: string }[];
}

/** Adopting an existing local folder (#129). */
interface AdoptExistingInstall {
  gameId: number; title: string; source: string; installPath: string;
}

interface AdoptPreview {
  folder: string;              // canonical folder that would be stored
  executable: string | null;   // discovery choice; null when inconclusive
  executableCandidates: string[]; // ranked files found (#134)
  sizeBytes: number;
  fileCount: number;
  versionId: number;           // 0 = unknown, never guessed from the title
  version: string | null;
  warnings: string[];          // includes unreadable folders and depth notes
  existing: AdoptExistingInstall | null; // needs replace: true when not adopted
}

interface DeleteAdoptedPreview {
  folder: string; sizeBytes: number; fileCount: number;
  deleted: boolean;            // false for the preview call
  missing: boolean;            // folder already gone; only the row was removed
  warnings: string[];
}

interface ManifestReport { gameId: number; files: number; bytes: number; builtAt: string }

interface ManifestProgress {
  gameId: number; filesDone: number; filesTotal: number;
  bytesDone: number; bytesTotal: number; message: string;
}

type GameVaultMatchStatus = "exact" | "containment" | "ambiguous" | "none";

interface GameVaultCandidate {
  folder: string; name: string;
  sourceRoot: string;                // configured root it was found under (#134)
  executable: string | null;         // null = "link anyway and choose" (#134)
  executableCandidates: string[];    // ranked, best first (#134)
  sizeBytes: number; fileCount: number;
  metadataTitle: string | null;      // read from gamevault.json, never written
  matchedGameId: number | null; matchedTitle: string | null;
  matchStatus: GameVaultMatchStatus; // ambiguous is never adopted automatically
  installed: string | null;          // adopted / ludex / launcher id
  notes: string[];                   // depth, skipped subfolders, chosen exe…
  warnings: string[];                // unreadable folder, broken link, long path
}

interface GameVaultRootScan {
  root: string;
  candidates: number;
  warnings: string[];
  error: string | null;              // root unreadable or refused
}

interface GameVaultRootProposal {
  path: string;
  reason: string;                    // usual location, GameVault config file…
  exists: boolean;
  configured: boolean;               // already in the ordered list
  gameFolders: number;               // immediate folders that look like games
}

interface GameVaultScan {
  roots: GameVaultRootScan[];        // one per configured root, in order (#134)
  candidates: GameVaultCandidate[];  // deduplicated by canonical folder
  warnings: string[];
  matched: boolean;                  // false: the server could not be listed
  maxDepth: number;                  // folder depth used for this scan
}

interface GameVaultAdoptEntry {
  folder: string; gameId: number;
  versionId?: number | null; version?: string | null;
  executable?: string | null;        // chosen among the ranked candidates
}

interface GameVaultAdoptReport {
  adopted: InstalledGame[];
  skipped: { folder: string; reason: string }[];
}

/** How a game is started on Linux (#89); stored per game in the desktop index. */
interface LaunchOptions {
  mode: "auto" | "native" | "proton" | "umu";
  toolId: string;                    // compat_scan tool id, empty = best available
  prefixPath: string;                // empty = {installPath}/.prefix (#92)
  env: Record<string, string>;       // DXVK_*, PROTON_*, WINEDLLOVERRIDES…
  winetricksVerbs: string[];         // run on demand in the game prefix
  steamAppId: number | null;         // ProtonDB (#91) and GAMEID
}

/** One detected Steam/Proton/UMU/Wine tool (#90). */
interface CompatTool {
  id: string;                        // "umu:<path>", "proton:<dir>", "wine:<path>"
  name: string;
  kind: "umu" | "proton" | "wine";
  version: string | null;            // Proton `version` file when present
  path: string;                      // umu-run, proton script or wine binary
  protonPath: string | null;         // Proton folder (PROTONPATH)
  source: string;                    // "path" | "steam" | "compatibilitytools.d" | "override"
  steamRoot: string | null;
}

interface CompatScan {
  platform: string; linux: boolean;
  steamRoots: string[];              // libraries whose Proton builds were read
  tools: CompatTool[];               // UMU first, then GE/Proton, then Wine
  umu: string | null; proton: string | null; wine: string | null;
  winetricks: string | null; protonfixes: string | null;
  defaultMode: "auto" | "native" | "proton" | "umu";
  steamClientPath: string | null;    // STEAM_COMPAT_CLIENT_INSTALL_PATH
  suggestedVerbs: string[];
  warnings: string[];
}

interface PrefixInfo {                // #92
  gameId: number; path: string; custom: boolean;
  exists: boolean; sizeBytes: number;
}

interface WinetricksReport {
  verbs: string[]; success: boolean; timedOut: boolean;
  exitCode: number | null; output: string; durationMs: number;
}

/** ProtonDB summary (#91), cached seven days in the desktop index. */
interface ProtonDbRating {
  appId: number | null;
  status: "ok" | "cached" | "stale" | "unavailable" | "notFound" | "noAppId";
  tier: "platinum" | "gold" | "silver" | "bronze" | "borked" | "pending" | null;
  bestTier: string | null; trendingTier: string | null; confidence: string | null;
  total: number; score: number | null;
  url: string | null;                // https://www.protondb.com/app/<id>
  fetchedAt: string | null; message: string | null;
}

/** A game with a newer version on the server (#30). */
interface GameUpdate {
  gameId: number; title: string;
  installedVersionId: number; installedVersion: string | null;
  latestVersionId: number; latestVersionLabel: string | null;
  archiveSize: number; archiveHash: string | null; updatedAt: string | null;
  reason: "newVersion" | "replacedArchive";
}

interface PlaySession {
  id: string; gameId: number; startedAt: string; endedAt: string | null;
  minutes: number; seconds: number;
}

interface GamePlaytime {
  gameId: number; totalSeconds: number; sessions: number;
  firstPlayedAt: string | null; lastPlayedAt: string | null;
  byDay: { day: string; seconds: number }[];   // UTC days, newest first, 30 days
  recentSessions: PlaySession[];               // newest first
}

interface UpdateStatus {
  configured: boolean;              // false: build has no signing key
  channel: "stable" | "early";
  currentVersion: string;
  lastCheckedAt: string | null;
  endpoint: string;                 // GitHub latest.json the check used (#163)
  skippedVersion: string | null;    // #163: prompt/badge silenced for this version
  notify: boolean;                  // false when the version is skipped, or none is available
  available: { version: string; currentVersion: string; notes: string | null;
               publishedAt: string | null; releaseUrl: string; // (#163)
               isPrerelease: boolean; build: number | null } | null;
  message: string | null;
}

interface InstallJob {
  jobId: string; gameId: number; versionId: number;
  phase: "queued" | "downloading" | "extracting" | "finalizing" | "paused"
       | "done" | "cancelled" | "error";
  bytesDone: number; bytesTotal: number | null; speedBps: number;
  message: string | null; installPath: string | null;
  stream: boolean;              // job used a stream install (#114)
  resumable: boolean;           // per-entry checkpoint exists and can resume (#115)
  entriesDone: number | null;   // per-entry stream installs
  entriesTotal: number | null;
  currentEntry: string | null;  // entry currently being written
  resumedEntries: number | null;// files already present when this attempt started
}

interface VerifyIssue {
  path: string;                 // relative to the install folder, "/" separated
  expectedSize: number; actualSize: number | null;
  expectedSha256: string; actualSha256: string | null;
}

interface VerifyReport {
  gameId: number;
  status: "ok" | "issues" | "noManifest";
  filesChecked: number; bytesChecked: number;
  missing: VerifyIssue[]; corrupt: VerifyIssue[];
  extra: { path: string; size: number }[];
  checkedAt: string; message: string | null;
}

interface RepairReport {
  gameId: number;
  repaired: string[];           // restored relative paths
  failed: string[];             // damaged paths the archive could not restore
  fallback: boolean;            // true: a full reinstall is required
  message: string;
  verify: VerifyReport;         // state after the repair
}
```

Cloud save types (#108):

```ts
type CloudSaveReason = "on" | "global_off" | "game_off";
type CloudSaveStrategy = "remote" | "keep_local";

interface CloudSaveStatus {
  gameId: number; serverConfigured: boolean;
  paths: { path, kind, primary, exists, files, sizeBytes, lastModified }[];
  localFiles: number; localBytes: number; localNewestMtime: string | null;
  latest: SaveSnapshot | null;
  lastHash: string | null; lastUploadAt: string | null; lastRestoreAt: string | null;
  toggle: { enabled: boolean; reason: CloudSaveReason };
  conflict: {
    localNewestMtime: string | null; lastUploadAt: string | null;
    lastHash: string | null; remoteSha256: string;
    remoteCreatedAt: string; remoteDeviceId: string | null;
  } | null;
  pending: { pending: boolean; attempts: number; nextAttemptAt: string | null; error: string | null };
  deviceId: string | null;
  /** ISO timestamp of the last save detection, null when it never ran (#158). */
  lastDetectionAt: string | null;
  message: string | null;
}

interface CloudSaveRestoreOutcome {
  restored: boolean; strategy: CloudSaveStrategy;
  snapshotId: number | null; backupPath: string | null; message: string;
}

interface CloudSyncToggleResponse {
  gameId: number | null; globalEnabled: boolean;
  effective: { enabled: boolean; reason: CloudSaveReason };
}

/** Auto-managed Ludusavi manifest (#153). `path`, `source` and the file itself
 *  stay on the desktop machine and are never sent to the server. */
interface ManifestStatus {
  present: boolean;
  path: string | null;          // {appDataDir}/manifest/ludusavi-manifest.yaml
  version: string | null;       // "sha256:<12 hex>"
  updatedAt: string | null;     // last successful download, RFC 3339
  checkedAt: string | null;     // last check, successful or not
  source: string;               // effective download URL (official or mirror)
  sizeBytes: number;
  autoUpdate: boolean;          // ludusaviManifestAutoUpdate
  stale: boolean;               // older than 24 h, or never checked
  error: string | null;         // last download error; the cached copy is kept
  message: string;
}

interface ManifestUpdate extends ManifestStatus {
  updated: boolean;             // the cached file was replaced
  notModified: boolean;         // the server answered 304
}

/** Game settings editor (#105/#147). `filePath` is relative to the install folder.
 *  `editable: false` means the UI never calls `read_game_config`; `note` gives
 *  the reason (binary/NUL bytes, over 1 MiB, unparseable, unsupported encoding,
 *  path escaping the install folder). No filename blacklist. */
interface ConfigFileInfo {
  path: string; name: string;
  format: "ini" | "cfg" | "toml" | "json";
  sizeBytes: number; entries: number; editable: boolean; note: string | null;
}
interface ConfigEntry {
  key: string;                  // Section/key (INI), section.key (TOML), a.b[0] (JSON)
  value: string;
  valueType: "string" | "number" | "boolean";
  section: string | null; line: number;
}
interface ConfigDocument {
  path: string; name: string; format: ConfigFileInfo["format"];
  sizeBytes: number; entries: ConfigEntry[]; note: string | null;
}
interface ConfigBackupRecord {
  id: string; gameId: number; filePath: string;
  backupPath: string;           // local machine only
  diff: string; bytes: number;
  origin: "manual" | "adapter" | "restore" | "common-field" | "sync";
  createdAt: string;            // RFC 3339
}
/** Common config fields (#148). Detection is read-only. An ambiguous match has
 *  no file/key/value: the UI lists `candidates` and never auto-picks. */
interface CommonFieldCandidate {
  file: string; key: string; value: string;
  valueType: "string" | "number" | "boolean";
  line: number; section: string | null;
  exact: boolean;               // key itself matched, not just its section
}
interface CommonFieldMatch {
  field: "language" | "playerName"; label: string;
  confidence: "certain" | "likely" | "ambiguous";
  source: "builtin" | "template"; // catalog heuristic vs pinned per-game mapping
  file: string | null; key: string | null; value: string | null;
  valueType: ConfigEntry["valueType"] | null;
  allowedValues: string[];      // values the file itself lists (GOG langs[n])
  candidates: CommonFieldCandidate[];
  note: string | null;
}
interface CommonFieldsReport {
  gameId: number; installPath: string;
  fields: CommonFieldMatch[];   // empty = no panel
  mappings: ConfigFieldMapping[];
  notes: string[];
}
interface ConfigFieldMapping {
  gameId: number;
  field: "language" | "playerName";
  file: string; key: string;    // relative to the install folder
  createdAt: string;
}
interface CommonFieldPreview {
  field: "language" | "playerName";
  file: string; key: string;
  currentValue: string; nextValue: string;
  unchanged: boolean; diff: string;
}
interface ConfigSyncPrefs {
  gameId: number; enabled: boolean;
  language: string; playerName: string; // last values handed over by the UI
  applyLanguage: boolean; applyPlayerName: boolean;
  lastAppliedAt: string | null; updatedAt: string;
}
interface AdapterField {
  id: string; label: string;
  control: "select" | "toggle" | "number" | "text";
  value: string | null;
  options: { value: string; label: string }[];
  file: string | null; key: string | null; // null = read-only / outside the install folder
  valueType: ConfigEntry["valueType"];
  note: string | null;
}
interface AdapterReport {
  gameId: number; installPath: string;
  adapters: Array<{
    id: "unreal" | "gog" | "godot" | "unity";
    label: string; detected: boolean; evidence: string | null;
    files: { path: string; label: string; exists: boolean }[];
    fields: AdapterField[]; notes: string[];
  }>;
  notes: string[];
}

/* -------------------- Multiple servers and accounts (#24) ----------------- */

interface ServerProfile {
  id: string;                         // "local" for the embedded server, else srv_<hash>
  label: string; url: string;
  username: string | null;            // a local reference, not a credential
  lastUsedAt: string | null; createdAt: string | null;
  active: boolean; local: boolean;
  hasToken: boolean;                  // a token is present in the secret store
}

interface ServerList {
  servers: ServerProfile[];
  activeId: string | null;
  tokenStorage: "keychain" | "database"; // where saved profile tokens live
  keychain: string | null;            // "secret-service" | "macos-keychain"
  warning: string | null;             // set while the SQLite fallback is used
  message: string | null;
}

interface ServerTest {
  ok: boolean; status: number | null; version: string | null;
  latencyMs: number; message: string;
}

/* ------------------ Sunshine host (#97) and Moonlight (#98) --------------- */

interface SunshineStatus {
  detected: boolean; platform: string;
  appsPath: string | null; confPath: string | null; binary: string | null;
  source: string | null; overridden: boolean; appsFound: boolean;
  entries: number; managedEntries: number;
  warnings: string[]; message: string;
}

interface SunshinePreviewEntry {
  gameId: number; title: string; name: string; cmd: string;
  workingDir: string; imagePath: string | null;
  action: "add" | "update" | "unchanged"; needsLudexScheme: boolean;
}

interface SunshinePreview {
  status: SunshineStatus;
  entries: SunshinePreviewEntry[];
  removals: { name: string; gameId: number | null }[];
  skipped: { gameId: number; title: string; reason: string }[];
  canSync: boolean; message: string;
}

interface SunshineReport {
  written: boolean; dryRun: boolean;
  appsPath: string | null; backupPath: string | null;
  added: number; updated: number; unchanged: number; removed: number;
  managedEntries: number;
  skipped: { gameId: number; title: string; reason: string }[];
  message: string;
}

interface MoonlightHost {
  name: string; address: string;
  localAddress: string | null; remoteAddress: string | null;
  manualAddress: string | null; paired: boolean;
}

interface MoonlightStatus {
  detected: boolean; platform: string;
  binary: string | null; configPath: string | null;
  configSource: string | null;       // Moonlight.ini, Windows registry, Moonlight.conf, override (#151)
  configFound: boolean;
  manualHost: string | null;         // moonlightHost fallback (#151)
  hosts: MoonlightHost[]; supported: boolean;
  warnings: string[]; message: string;
}

interface StreamLaunch {
  launched: boolean; host: string; app: string;
  command: string[]; message: string;
}

interface StreamTarget {
  published: boolean; appName: string | null; gameId: number;
  hosts: MoonlightHost[]; moonlight: MoonlightStatus; sunshine: SunshineStatus;
  message: string;
}
```

| Command | Args | Returns |
|---|---|---|
| `get_settings` | — | `LocalSettings` |
| `save_settings` | `{ settings }` | `LocalSettings`; also re-applies the window decorations when `nativeTitleBar` changed (#111) |
| `set_native_title_bar` | `{ enabled }` | `LocalSettings`; shows the OS title bar (`true`) or Ludex's own (`false`) on the live window and persists it, no restart. No-op decoration change on macOS (#111) |
| `pick_folder` | `{ title? }` | `string \| null` |
| `local_server_start` | — | `{ url, token }` |
| `local_server_stop` | — | `null` |
| `local_server_status` | — | `{ running, url }` |
| `install_game` | `{ gameId, versionId, serverUrl, token, title, version?, executable?, installDir?, downloadDir?, locationId?, downloadLocationId?, expectedBytes?, stream? }` | `InstallJob` (starts background; survives dialog close). `locationId`/`downloadLocationId` pick #99 named locations; `installDir` is the direct-root override used while a #145 recommendation is not saved yet; `expectedBytes` enables the free-space guard. An unknown id or a read-only/missing location fails immediately |
| `cancel_install` | `{ jobId }` | `null` |
| `pause_install` | `{ jobId }` | `InstallJob` (`paused`; keeps the `.part`) |
| `resume_install` | `{ jobId }` | `InstallJob` (continues with HTTP Range) |
| `get_install_job` | `{ jobId }` | `InstallJob` |
| `list_install_jobs` | — | `InstallJob[]` (active + persisted, newest first) |
| `delete_install_job` | `{ jobId }` | `null`; removes one terminal (`done`, `cancelled`, `error`, `interrupted`) row from Downloads history. Refuses active and paused jobs. Never deletes the install folder, archives or `.part` files |
| `clear_finished_install_jobs` | — | `number` (rows removed); deletes every terminal history row, keeps active/paused jobs and all files on disk |
| `list_installed` | — | `InstalledGame[]` |
| `launch_game` | `{ gameId, executable?, args? }` | `{ sessionId }` |
| `stop_game` | `{ sessionId }` | `null` |
| `uninstall_game` | `{ gameId }` | `null` |
| `move_install` | `{ gameId, targetDir }` | `InstalledGame` (new path) |
| `verify_install` | `{ gameId }` | `VerifyReport` |
| `repair_install` | `{ gameId }` | `RepairReport` |
| `open_path` | `{ path }` | `null` |
| `get_disk_usage` | `{ path }` | `{ totalBytes, freeBytes }` |
| `list_recent_games` | `{ limit? }` | `InstalledGame[]`, newest launch first (default 8, max 50) |
| `get_game_playtime` | `{ gameId, days?, sessions? }` | `GamePlaytime` (defaults: 30 days, 10 sessions) |
| `check_game_updates` | — | `GameUpdate[]`; per-game failures are skipped |
| `update_game` | `{ gameId }` | `InstallJob`; installs the newest server version, keeps the previous install until the new files are in place |
| `check_for_updates` | `{ manual? }` | `UpdateStatus`; manual checks bypass the 24 h throttle. A skipped version keeps `available` but sets `notify: false` (#163) |
| `install_update` | — | `UpdateStatus` after download+install; refuses when no signing key is configured. Signature, disk and network failures return a specific message (#163) |
| `restart_app` | — | `null` (restarts Ludex; Linux/macOS after an update) |
| `update_status` | — | `UpdateStatus` without a network check: current version, channel, endpoint, skipped version, last check (#163) |
| `set_update_channel` | `{ channel }` | Persists `updateChannel` then re-checks on it; unknown values are rejected (#163) |
| `set_update_skip` | `{ version }` | Persists `skippedUpdateVersion` (`null` clears it) and returns the status without a network check. Silences prompts/badges only (#163) |
| `installer_plan` | `{ gameId?, executable? }` | `InstallerPlan`; never runs anything. On Linux `compatTool`/`prefix` show the tool a run would use |
| `run_installer` | `{ gameId, executable?, interactive?, args? }` | `InstallerReport`; Windows runs `setup.exe` directly, Linux through UMU/Proton/Wine (#93); interactive runs use no arguments and no timeout |
| `cancel_installer` | `{ gameId }` | `null`; stops a running installer at the next poll |
| `set_executable` | `{ gameId, executable }` | `InstalledGame`; the file must be inside the install folder or its Wine prefix |
| `choose_executable` | `{ gameId }` | `InstalledGame \| null`; system file dialog, records the pick |
| `list_game_executables` | `{ gameId, refresh? }` | `GameExecutables`; scans the game's install folder (depth 4-8, default 6, bounded, loop-safe) for the Launch options pickers. `entries` are `{ path, name, sizeBytes, modifiedAt, kind: "launch" \| "installer" \| "uninstaller" \| "other", recommended }`; `path` is relative to `installPath` and never leaves it, `recommended` marks the `pick_executable` result. Installer kinds are `setup*`, `install*`, `gog*`; uninstallers are `unins*`, `uninstall*`. Works for `ludex` installs, adopted folders and launcher imports that have a folder; a row without a usable folder errors. Cached for 5 s, `refresh: true` rescans (#143) |
| `check_game_executable` | `{ gameId, path }` | `string`: the normalised forward-slash relative path. Resolves the value inside the install folder and rejects `..`, absolute paths outside it and symlink escapes; used before saving the server launch config (#143) |
| `compat_scan` | — | `CompatScan`; detects Steam roots, Proton builds, UMU, Wine, winetricks and protonfixes (#90) |
| `get_launch_options` | `{ gameId }` | `LaunchOptions`; defaults when nothing was saved (#89) |
| `set_launch_options` | `{ gameId, options }` | `LaunchOptions`; validates env names, verbs and prefix (#89, #92) |
| `wine_prefix_info` | `{ gameId }` | `PrefixInfo`; path, custom flag, existence and size (#92) |
| `reset_wine_prefix` | `{ gameId }` | `null`; deletes the game prefix after UI confirmation, refuses while the game runs |
| `run_winetricks` | `{ gameId, verbs? }` | `WinetricksReport`; runs `winetricks -q <verbs>` with the game prefix, 30 min timeout (#92) |
| `protondb_rating` | `{ gameId, appId?, refresh? }` | `ProtonDbRating`; cached 7 days, never fails offline (#91) |
| `presence_status` | — | `PresenceStatus` (never blocks on the IPC socket) |
| `steam_scan` | — | `SteamPreview`; reads Steam metadata, matches titles, marks imports |
| `steam_import` | `{ appIds }` | `SteamImportReport`; no download, launches through Steam |
| `launcher_scan` | `{ sources?: LauncherGame["source"][] }` | `LauncherPreview`; reads Steam, Epic, GOG, EA, Ubisoft, Xbox, itch, Heroic and Lutris metadata (#116), matches titles and marks existing imports. `sources` is the enabled set (#155): a source outside it is never read and is reported with `disabled: true`; omitting it uses `launcherSources` from the settings |
| `launcher_import` | `{ entries: LauncherImportEntry[], sources?: LauncherGame["source"][] }` | `LauncherImportReport`; writes external rows with `launchUri`, `versionId: 0`, `source` = launcher id. Nothing is downloaded or copied; a second import of the same URI is refused, and an entry from a disabled source is skipped with a reason (#155) |
| `adopt_installation` | `{ gameId, folder, versionId?, version?, executable?, title? }` | `AdoptPreview`; canonicalizes and validates the folder, runs read-only executable discovery, returns ranked candidates, size/file count/warnings and any existing row. Writes nothing (#129, #134) |
| `confirm_adopt_installation` | `{ gameId, folder, versionId?, version?, executable?, replace?, title? }` | `InstalledGame`; writes the `source: "adopted"` row with `installRoot` = the folder's parent and `archiveHash: null`. Nothing is copied or moved. Replacing a `ludex`/launcher row requires `replace: true`; launcher-owned folders are refused |
| `build_install_manifest` | `{ gameId }` | `ManifestReport`; hashes the folder into `install_manifests` (progress on `install://manifest-progress`). No file is written inside the game folder |
| `unlink_installation` | `{ gameId }` | `null`; removes the index row, manifest and detected save paths. Files stay on disk |
| `delete_adopted_installation` | `{ gameId, confirmPath? }` | `DeleteAdoptedPreview`; without `confirmPath` it only counts files/size. With a `confirmPath` exactly equal to the canonical stored path it deletes the folder and the row. Refuses non-adopted rows, symlinks and folders directly inside a drive/home/install root |
| `scan_gamevault_folders` | `{ roots?: string[] }` | `GameVaultScan`; scans every configured root (or the given list) in one pass, with per-root status. Lists immediate subfolders that contain an executable (depth 4-8, default 6), match a library title or carry GameVault metadata; a matched folder is never dropped when no executable is found. Deduplicates by canonical folder and matches titles by exact normalised title, then word-boundary containment. Ambiguity is reported, never guessed (#134) |
| `adopt_gamevault_installations` | `{ roots: string[], entries: GameVaultAdoptEntry[] }` | `GameVaultAdoptReport`; bulk adoption of selected candidates after confirmation. Every folder must be an immediate child of one of the roots; per-entry failures are skipped with a reason |
| `add_gamevault_root` | `{ path }` | `LocalSettings`; canonicalizes the folder, refuses the home/drive root and duplicates, appends it to `gamevaultRoots` (#134) |
| `remove_gamevault_root` | `{ path }` | `LocalSettings`; removes the matching canonical root (#134) |
| `reorder_gamevault_roots` | `{ roots: string[] }` | `LocalSettings`; persists the new order, ignoring unknown entries and keeping unlisted roots at the end (#134) |
| `detect_gamevault_roots` | — | `GameVaultRootProposal[]`; read-only probe of the usual GameVault locations and GameVault's client config files. Never writes to GameVault (#134) |
| `detect_save_paths` | `{ gameId }` | `SavePaths`; reads the manifest (explicit path → managed copy → user install), then the optional Ludusavi CLI, merges persisted manual paths and saves the mapping + preview. `saves` entries are `{ path, kind: "file" \| "directory", source: "manifest" \| "manual" \| "override", primary, sizeBytes, lastModified, notes }`; `detected: false` plus `message` is the explicit "no save folder detected" state. Paths stay local |
| `redetect_save_paths` | `{ gameId }` | `SavePaths`; same as `detect_save_paths`, for a re-run after the game created its saves |
| `get_save_paths` | `{ gameId }` | `SavePaths` from SQLite + last preview; never rescans the disk |
| `set_save_paths` | `{ gameId, paths: { path, kind?, primary?, source? }[] }` | `SavePaths`; replaces the per-game manual override list, checks each path is readable and inside an allowlisted root, allows at most one primary. `source` is `manual` (default) or `override` |
| `ludusavi_manifest_status` | — | `ManifestStatus`; cached save-path manifest (`{appDataDir}/manifest/ludusavi-manifest.yaml`), version, last updated/checked, size and effective source URL. Local only (#153) |
| `update_ludusavi_manifest` | — | `ManifestUpdate`; downloads or refreshes the manifest now, ignoring the 24 h throttle. Conditional `If-None-Match`, 64 MiB cap, 60 s timeout, optional SHA-256 check; a failure keeps the cached copy. No tests hit the network (#153) |
| `remove_ludusavi_manifest` | — | `ManifestStatus`; deletes the cached manifest and its metadata so detection falls back to a user-installed copy or manual paths (#153) |
| `cloud_save_status` | `{ gameId }` | `CloudSaveStatus`; detected paths, latest snapshot (remote failures go in `message`), last upload/restore, effective toggle + reason, pending retry, conflict and `lastDetectionAt` (#108, #158). The local walk is cached and bounded (200 ms / 4 000 files) and runs on the blocking pool, so opening a game page never scans the whole disk (#159) |
| `set_cloud_save_sync` | `{ gameId?, enabled }` | `CloudSyncToggleResponse`; `gameId` null changes the global default, a number stores a per-game override. Local only |
| `sync_cloud_save_now` | `{ gameId }` | `CloudSaveStatus`; archives and uploads immediately. Works with automatic sync off; an unchanged hash is a no-op. Failures stay in `pending` |
| `restore_cloud_save` | `{ gameId, strategy: "remote" \| "keep_local" }` | `CloudSaveRestoreOutcome`; verifies the SHA-256, backs up newer local files before a `remote` restore and returns `backupPath`, runs at most once per snapshot. `keep_local` records and uploads the local state (#108) |
| `resolve_cloud_save_conflict` | `{ gameId, strategy }` | `CloudSaveRestoreOutcome`; same operation as `restore_cloud_save`, using the explicit strategy |
| `list_locations` | — | `LocationStatus[]`; probes every named location, default first then roomiest writable. Never fails on a missing drive |
| `check_location` | `{ id?, path?, requiredBytes? }` | `LocationStatus`; one location by id or path, with `freeSpaceOk` when `requiredBytes` is set |
| `suggest_storage_locations` | — | `StorageSuggestions`; detected fixed drives as `<drive>:\Games` candidates on Windows (`~/Games`, `/mnt/...`, `/media/...`, `/Volumes/...` elsewhere), with free/total bytes, writability, `recommended`/`current`/`fallback` flags and `needsChoice`. Read-only: never creates a folder or changes settings (#145) |
| `list_game_configs` | `{ gameId }` | `{ gameId, installPath, files: ConfigFileInfo[] }`; every ini/cfg/conf/godot/prefs/toml/json/info file under the install folder (depth ≤5, 200 files). No filename blacklist; read-only entries carry `editable: false` and a `note` (#147) |
| `read_game_config` | `{ gameId, path }` | `ConfigDocument`; parsed entries with byte-preserving spans, never writes. Decodes UTF-8 (±BOM), UTF-16LE/BE (BOM) and Windows-1252 |
| `write_game_config` | `{ gameId, path, edits: { key, value }[], origin? }` | `{ document, diff, backup }`; validates types, copies the original to `{appDataDir}/config-backups/game-<id>/`, records the diff, then replaces the file atomically in its original encoding/BOM. Refuses binary/NUL files, files over 1 MiB, values unrepresentable in the source encoding and paths outside the install folder |
| `restore_game_config` | `{ gameId, backupId }` | `{ document, diff, backup }`; backs up the current file first, then restores the history entry byte-for-byte |
| `list_game_config_backups` | `{ gameId }` | `ConfigBackupRecord[]`, newest first (max 100) |
| `game_config_adapters` | `{ gameId }` | `AdapterReport`; Unreal/GOG/Godot/Unity detection plus typed fields (#106) |
| `game_config_common_fields` | `{ gameId }` | `CommonFieldsReport`; read-only detection of the language/player-name keys across the editable config files. Reports `confidence` (`certain`/`likely`/`ambiguous`), the source (`builtin` catalog or pinned `template`), the exact file/key and the file's allowed values. Ambiguous matches return candidates with no chosen key; no match returns `fields: []` (#148) |
| `preview_game_config_common_field` | `{ gameId, file, key, target: { field, value } }` | `CommonFieldPreview`; normalises the value (`fr`/`French`/`Français`, allowed values, player-name rules), validates the file's encoding and returns the before/after diff. Never writes |
| `apply_game_config_common_field` | `{ gameId, file, key, target }` | `{ document, diff, backup }`; same normalisation, then the regular backup + atomic write with `origin: "common-field"` |
| `save_game_config_field_mapping` / `delete_game_config_field_mapping` | `{ gameId, field, file, key }` / `{ gameId, field }` | `CommonFieldsReport` after the change. A pinned file/key overrides the catalog; re-pinning replaces it. Deleting returns to heuristics (#148) |
| `export_game_config_mappings` | `{ gameId }` | `string`; pretty JSON `{ gameId, exportedAt, mappings[] }` for saving/importing a template (#148) |
| `game_config_sync_prefs` / `set_game_config_sync_prefs` | `{ gameId }` / `{ gameId, prefs: { enabled, language?, playerName?, applyLanguage?, applyPlayerName? } }` | `ConfigSyncPrefs`; the per-game opt-in that re-applies the values at launch without a dialog. Off by default; toggling it off stops all writes (#148) |
| `set_config_sync_defaults` | `{ language, playerName }` | `LocalSettings`; mirrors the web locale/profile name so launch-time sync uses the current values even when the game page was never opened (#148) |
| `sunshine_status` | — | `SunshineStatus`; Sunshine install, `apps.json`/`sunshine.conf` paths, binary and managed-entry counts (#97) |
| `sunshine_preview` | — | `SunshinePreview`; what a sync would add, update and remove. Never writes |
| `sunshine_sync` | `{ dryRun?, removeMissing?, downloadCovers? }` | `SunshineReport`. Every real write copies `apps.json` to `apps.json.ludex-backup-<stamp>-<id>.bak` first. `removeMissing` defaults to true |
| `sunshine_remove_managed` | — | `SunshineReport`; removes every entry carrying the Ludex marker (all servers), user apps stay, backup first |
| `moonlight_status` | — | `MoonlightStatus`; binary, config source (portable `Moonlight.ini`, Windows registry, `Moonlight.conf`), parsed hosts and the manual host fallback (#98, #151) |
| `stream_target` | `{ gameId }` | `StreamTarget`; whether the game is published and which hosts can stream it |
| `stream_game` | `{ gameId, host? }` | `StreamLaunch`; runs `moonlight stream <host> <app>` for a published game. Refuses unpaired hosts and missing Moonlight |
| `moonlight_stream` | `{ host, appName }` | `StreamLaunch`; advanced/manual start of any Sunshine app |
| `list_servers` | — | `ServerList`; saved servers, active profile and token-storage backend (#24) |
| `save_server` | `{ id?, label, url, username?, token? }` | `ServerList`; validates and normalises the URL, stores the token in the secret store |
| `remove_server` | `{ id }` | `ServerList`; deletes the saved token. When the active profile is removed the first remaining one becomes active |
| `switch_server` | `{ id }` | `ServerList`; refused while a game runs or an install is active. Swaps the per-profile desktop index |
| `test_server` | `{ url, token? }` | `ServerTest`; `GET {url}/api/health` with an 8 s timeout. Uses the saved token when omitted |
| `oidc_login_start` | — | `{ sessionId, webLoginUrl, expiresAt }`; generates a 32-byte state and opens `{serverUrl}/#/login?desktop_state=…` in the system browser. A browser-open failure is a string error naming the fallback URL (#157) |
| `oidc_login_poll` | `{ sessionId }` | `{ status: "claimed" \| "expired" \| "cancelled" \| "error", code, message }`; polls the claim endpoint every second for up to five minutes, then the webview redeems `code` through the existing exchange. `code` is `null` unless `status` is `claimed`; the app never logs the state or the code (#157) |
| `oidc_login_cancel` | `{ sessionId }` | Stops an in-flight poll; idempotent, the attempt resolves as `cancelled` (#157) |
| `open_external_url` | `{ url }` | Opens an `http(s)` address in the system browser (the "Open web login" fallback). Refuses any other scheme (#157) |
| `diagnostics_status` | — | `{ enabled, installId, appVersion, platform, os, locale, channel, reports, unsent }`; generates the random install id on first read. Local capture is on, uploading is not (#171) |
| `set_diagnostics_enabled` | `{ enabled }` | Refreshed status. Enabling only records consent; disabling deletes the local queue and forgets the install id (#171) |
| `diagnostics_preview` | `{ reportId? }` | `{ reportId, payload }`; the exact JSON body, already redacted. Defaults to the newest report (#171) |
| `diagnostics_send` | `{ reportId }` | `{ reportId, sent: true }`; refuses with a string error while the toggle is off. There is no automatic send path (#171) |
| `diagnostics_delete_reports` | — | `null`; clears every report stored on the device (#171) |
| `diagnostics_record_error` | `{ kind, summary, command?, stack? }` | `null`; called by the web error boundary, unhandled-rejection listener and invoke wrapper. Stored locally only (#171) |

Events:

- `install://progress` → `InstallJob`
- `install://done` → `InstallJob`
- `install://error` → `{ jobId, message }`
- `install://jobs` → `InstallJob[]` (whole queue after any change)
- `install://move-progress` → `{ gameId, bytesDone, bytesTotal, message }`
- `install://verify-progress` → `{ gameId, filesDone, filesTotal, bytesDone, bytesTotal, message }`
- `install://repair-progress` → same shape plus `phase: "downloading" | "extracting" | "restoring" | "verifying"`
- `game://exited` → `{ sessionId, gameId, minutesPlayed, secondsPlayed, source }` (#129 adds `source`)
- `install://manifest-progress` → `{ gameId, filesDone, filesTotal, bytesDone, bytesTotal, message }` (#129)
- `cloud-save://synced` → `gameId` (automatic upload completed or was unchanged)
- `cloud-save://pending` → `gameId` (automatic upload failed and was queued for retry)
- `save-manifest://updated` → `ManifestUpdate` (the save database was refreshed, manually or at startup) (#153)
- `local-server://status` → `{ running, url }`
- `ui://navigate` → `string` (tray menu asked for a route, e.g. `/downloads`)
- `ui://launch` → `number` (tray "Recent games" item; the UI calls `launch_game`)
- `update://available` → `UpdateStatus` (startup check found a version; emitted
  for a skipped version too, with `notify: false` — listeners gate prompts and
  badges on that flag, #163)
- `update://progress` → `{ downloaded, total, percent, speedBps }`
- `update://installed` → `{ version, restarted }`
- `installer://done` → `{ gameId, success, exitCode, tool }` (#7, #93; `tool` is
  the Linux compatibility tool or `null`)
- `launch_game` returns `{ sessionId, tool, source }`; `tool` is the Linux
  compatibility tool name when one was used (#89), `source` is the install
  source (`ludex`, `adopted`, or a launcher id) since #129.

Native install layout: `{installDir}/{slug}/{versionLabel}/game files` plus a
desktop SQLite index at `{appDataDir}/servers/<profileId>/index.db` (installed
games, sessions with exact seconds, install jobs, file manifests, archive
fingerprints, resumable stream manifests); an installation without a saved
server still uses `{appDataDir}/ludex-desktop.db`, which the first profile
adopts by copy. Per-entry stream checkpoints live in
`install_stream_manifests` (one manifest per job/version) and the
`install_jobs.stream_completed` hex bitset (schema v9). Local archive extraction
uses `GET /api/games/{id}/versions/{vid}/local-path` when the embedded server is
in local mode; remote installs download with Range resume and delete the temp
archive by default (`keepArchives` setting false).

### Desktop integrations (#7, #12, #13, #14, #116)

- **Silent installers.** `run_installer` tries `installerSilentArgs` (default
  `/S`, `/silent`, `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART`, `--silent`) in
  order, treats exit codes `0` and `3010` as success, kills an attempt after
  `installerTimeoutSecs` and moves on. After success it records the game binary
  it finds (never the installer), including a binary the setup program wrote
  inside the Wine prefix. Windows runs `setup.exe` directly; Linux runs it
  through the game's UMU/Proton/Wine tool with the game prefix and captures the
  log (`logTail`); macOS refuses. An interactive run sends no arguments and has
  no timeout. Nothing runs without an explicit command.
- **Linux compatibility (#89, #90, #91, #92, #93).** `compat_scan` reports Steam
  roots, `compatibilitytools.d` and `steamapps/common` Proton builds,
  `umu-run`, system `wine`/`wine64`, `winetricks` and `protonfixes`; every path
  can be overridden in Settings → Compatibility. Per game, `launchOptions`
  chooses `auto | native | proton | umu`, a tool id, a Wine prefix (default
  `{installPath}/.prefix`), environment overrides and winetricks verbs.
  `auto` runs native files directly and Windows `.exe` through UMU, then the
  best GE-Proton/Proton build, then Wine; the error names what is missing when
  none is installed. UMU gets `PROTONPATH`, `GAMEID`, `UMU_LOG`,
  `STEAM_COMPAT_DATA_PATH`, `STEAM_COMPAT_CLIENT_INSTALL_PATH` and
  `WINEPREFIX`; Proton gets the Steam compatibility variables and
  `WINEPREFIX=<prefix>/pfx`; Wine gets `WINEPREFIX=<prefix>`. Playtime tracking
  is unchanged (the launched process tree is sampled). `protondb_rating` reads
  `protondb.com/api/v1/reports/summaries/<appid>.json`, caches summaries for
  seven days in the desktop SQLite index, returns stale data instead of failing
  offline, and supports a manual `steamAppId`. `wine_prefix_info`,
  `reset_wine_prefix` and `run_winetricks` manage the prefix; reset refuses
  while the game runs and never deletes an install folder or its ancestor.
- **Executable picker (#143).** The Launch options form offers the programs
  actually present in the install folder instead of a free-text path.
  `list_game_executables` returns them with `kind` (`launch`, `installer`,
  `uninstaller`, `other`), size, modification time and `recommended` (the
  `pick_executable` heuristic from #134). The launch picker preselects the
  detected program, marks it "Detected" and hides redistributables/support
  programs; separate filtered lists drive the installer and uninstaller fields.
  A "Browse folder" action opens the folder in the OS file manager and a manual
  relative path stays available as an advanced fallback (and for games with no
  folder on this device). Before saving, `check_game_executable` normalises the
  value and refuses traversal, absolute paths outside the folder and symlink
  escapes. The listing is cached for 5 s with an explicit refresh; moving,
  repairing or running an installer invalidates it.
- **Discord presence.** Optional, off by default, uses the local Discord IPC
  (application id, no bot token). Payload: game title, public cover URL
  (`https`, non-private host), session start; idle when nothing runs.
  Reconnects with a 4 s → 120 s backoff. `presence_status` exposes
  `connected`, `lastError` and `retryInSeconds`.
- **Launcher import (#13, #116).** `launcher_scan` reads Steam
  (`steamapps/libraryfolders.vdf` + `appmanifest_*.acf`), Epic
  (`LauncherInstalled.dat` + `.item` manifests), GOG (`galaxy-2.0.db` and the
  registry), EA app and Ubisoft Connect (registry), Xbox / Microsoft Store
  (`Get-AppxPackage` + `AppxManifest.xml`), itch (`butler.db`), Heroic
  (`store_cache` JSON and sideloaded apps) and Lutris (`pga.db`). Only entries
  reported as installed, with an existing folder when one is recorded, are
  listed; parsers are pure and covered by fixtures, and a missing or malformed
  file is a warning. Titles are matched by normalised title, then
  edition-stripped titles, against the server library; ambiguous titles are
  never guessed and can be mapped by hand in the preview. `launcher_import`
  writes `versionId: 0`, `source` = the launcher id and `launchUri` (steam://,
  com.epicgames.launcher://, goggalaxy://, origin://, uplay://,
  shell:AppsFolder\…, itch://caves/…/launch, heroic://launch/…,
  lutris:rungameid/…). Matched games keep their server `gameId`; unmatched ones
  get a deterministic negative id (Steam keeps `-appId`). The partial unique
  index on `launchUri` plus a scan-time check refuse duplicates and the same
  install folder reported by two clients. Entries without a documented launch
  id are flagged and accept a manual URI; `file:`, `javascript:` and `data:`
  schemes are refused. Uninstalling an import only removes the index entry,
  never the launcher's files. Playtime for negative ids is not synced to the
  server. The web Library merges this index with the server shelf: a matched
  import is shown once on its server card with a source badge, and unmatched
  imports appear under `Launcher imports` (Installed filter and launcher source
  chips included) with a local dialog offering Play, Open folder, Open in
  <launcher> and Remove from Ludex. `/game/<id>` only routes positive ids, so a
  negative local id never becomes a server game page (#154).
- **Launcher source selection (#155).** `LocalSettings.launcherSources` keeps
  the sources the user ticked in Settings → Integrations (all nine by default;
  a settings file written before #155 has no key and deserializes to the full
  list, an empty array means "none"). Both launcher commands accept the set
  explicitly, so an unsaved toggle already applies, and fall back to the
  setting when the argument is absent. A source outside the set is never
  probed — no manifest, registry key or database is opened — and its
  `LauncherSource` comes back with `disabled: true`, `found: 0` and a
  "Disabled in Settings → Integrations" detail, which the preview renders as
  `Disabled` instead of `Not detected`. Disabling never deletes or hides rows
  already imported from that source (the local index is untouched and the
  installed list does not filter by source); re-enabling restores its scan on
  the next run, no restart. The set is normalised on every save: known ids
  only, deduplicated, in display order.
- **Adopted installs (#129).** `adopt_installation` validates a folder the user
  points at (absolute, canonical, no `..`, no link in the chosen path, not a
  drive/home root or the install root, no symlink escaping the tree), runs
  read-only executable discovery (#134) and returns a preview (executable,
  ranked candidates, size, file count, version, warnings, existing row). Nothing
  is written until `confirm_adopt_installation`, and nothing is ever copied or
  moved. The row uses `source: "adopted"`, `installRoot` = the folder's parent,
  `versionId: 0` ("unknown") unless the user picked a server version — versions
  are never matched from the title — and `archiveHash: null`. One row per game:
  replacing a `ludex` or launcher row needs `replace: true`, a folder already
  owned by a launcher import (#116) is refused, and the same folder cannot back
  two entries. Launching uses the normal pipeline with the folder as working
  directory and process-tree playtime; `launch_game` and `game://exited` report
  the source. An adopted folder has no manifest, so `verify_install` returns
  `noManifest` until `build_install_manifest` hashes it (progress on
  `install://manifest-progress`); Repair still needs a server archive.
  `unlink_installation` drops the row/manifest/save paths and keeps the files;
  `delete_adopted_installation` previews (canonical path, size, file count) and
  deletes only with a `confirmPath` exactly equal to the stored canonical path,
  refusing non-adopted rows, links and folders directly inside a drive, home or
  install root. `uninstall_game` refuses adopted rows. Game updates are not
  offered for adopted rows: an update would replace files Ludex does not own.
  `scan_gamevault_folders` scans every configured GameVault root (#134) in one
  pass. Immediate subfolders are kept when they contain a plausible executable
  (searched to `gamevaultScanDepth`, default 6, clamped 4-8), match a library
  title or carry GameVault metadata. A matched folder is never dropped: when no
  executable is found it is returned with `executable: null`, an explicit note
  and the `.exe` files that were found, so the user can link it and choose the
  executable in the game settings. Per-candidate `warnings` carry unreadable
  folders, broken links and long-path errors with the affected path. Discovery
  follows junctions/symlinks with canonicalisation and a visited set (no loops),
  skips redist/support/tools/installer folders, and returns a ranked candidate
  list: an unrelated executable is never auto-picked among several. Duplicate
  roots and folders reachable from two roots are listed once. GameVault roots
  live in local settings as an ordered list (`gamevaultRoots`) with
  add/remove/reorder commands, and `detect_gamevault_roots` proposes usual
  locations and roots read from GameVault's config files without writing
  anything. `adopt_gamevault_installations` only accepts immediate children of
  one of the scanned roots.
- **Playnite.** The plugin lives in `integrations/playnite/` and is a pure API
  client; there is no dedicated endpoint.

### Locations (#99)

`locations` replaces the two single folders with named entries, each with a
`kind` (`install`, `download`, `temp`), a label, an absolute path and one
`default` flag per kind. `normalize` migrates an old `settings.json` containing
only `installDir`/`downloadDir` into one install and (when set) one download
location; the legacy fields then mirror the defaults for older bundles, and
`save_settings` maps an old-style folder patch onto the default location.

`list_locations` probes each folder: existence, writability (a temporary probe
file in the folder or its nearest existing parent), free/total bytes, and a
note for a missing drive. It never creates anything, and a missing location is
data, not an error: Settings still opens, and `install_game` fails that one job
with a clear message. Installs pin their resolved folders on the job, so a
download that pauses overnight survives a renamed location.

The free-space guard keeps 256 MiB in reserve. On the install volume it
requires the extracted size (or twice that when the archive shares the volume:
peak usage is archive + extracted copy); on a different download volume it
requires the archive size there. `expectedBytes` is optional so old callers keep
working. Extraction always stages inside the install root, so a cross-volume
download is read from the download folder and unpacked directly into the
install volume; the archive is deleted afterwards unless `keepArchives`.

### Suggested install roots (#145)

`suggest_storage_locations` proposes drive-based roots instead of only
`{appDataDir}/Games`: every fixed drive as `<drive>:\Games` on Windows
(`GetLogicalDrives` + `GetDriveTypeW`, so network and removable drives stay
out by default), and `~/Games`, `/mnt/...`, `/media/...` (Linux) or
`/Volumes/...` (macOS) plus the app-data folder elsewhere. Each suggestion
carries free/total bytes, writability, `recommended`, `current`, `fallback`
and `nearlyFull` (below 10 GiB free or 10% of the volume). It is read-only:
no folder is created and `settings.json` is not touched.

The recommendation is the current default install root when the user already
has one, otherwise the writable non-fallback candidate with the most free
space, otherwise the app-data fallback. `needsChoice` mirrors
`installRootConfirmed`, a desktop-only settings flag that starts `false` and is
set by any edit of the Settings → Storage folder list (including the
detected-drive quick-add); while it is false the install dialog
preselects the recommended drive and passes its path as the legacy
`installDir` (the saved locations are unchanged). Settings → Storage lists the
candidates with checkboxes and install/download/scratch roles; "Add selected"
only appends locations and clears the stale default of that kind, so an
existing installation is never moved. A settings file written before #145 has
no `installRootConfirmed` key and keeps its `installDir` and locations
untouched.

### Game settings editor (#105) and adapters (#106)

Config files are discovered up to five folders deep inside the install folder,
up to 200 files per scan. The scanner has no filename or directory blacklist
(#147): every `ini`, `cfg`, `conf`, `godot`, `prefs` (including Unity's
extension-less `prefs`), `toml`, `json` and `info` file is listed, such as
`steam_emu.ini` or `Cracks/…/settings.ini`. Only scan-safety skips remain:
hidden folders, Wine `drive_c` prefixes and symlinks. Each parser records the
byte span of a value token; an edit replaces only that span, so comments,
ordering, quoting style and unrelated whitespace survive. Values are validated
against the existing type (string, number, boolean) and newlines/control
characters are refused.

Text is decoded as UTF-8 (with or without BOM), UTF-16LE/BE (with BOM) or, when
the bytes are neither valid UTF-8 nor contain NULs, Windows-1252. A save
re-encodes with the same encoding and BOM and keeps CRLF/LF exactly; a value
the source encoding cannot represent (for example CJK text in a Windows-1252
file) is refused before anything is written. A file is listed read-only
(`editable: false` with a `note`) when it contains NUL bytes or cannot be
decoded, is over 1 MiB, does not parse in its format, or its path (symlinks
resolved) leaves the install folder; the UI shows the note and never calls
`read_game_config` for those entries.

Every write copies the original to
`{appDataDir}/config-backups/game-<gameId>/<timestamp>-<uuid>-<name>` first,
then replaces the file atomically (temp file in the same folder + rename,
preserving the Unix mode). `config_edits` stores the relative path, absolute
backup path, line diff, size, origin and timestamp. Restoring a backup first
copies the current file to a new `restore` backup, so a restore is undoable too,
and writes the backup bytes verbatim (UTF-16 and Windows-1252 files included).

Adapters are detection-only: `game_config_adapters` scans the install folder for
markers (`*.uproject`, `*.pak`, `Engine/`, `goggame-*.info`, `project.godot`,
`*.pck`, `*_Data/globalgamemanagers`, `UnityPlayer.dll`, `prefs`) and maps
resolution, window mode, V-Sync, frame limit, quality and language to typed
fields with the exact file and key. A field whose storage lives outside the
install folder (Windows registry, UE `%LOCALAPPDATA%`, Godot `user://`) is
returned read-only with a note instead of being edited.

Common fields (#148) build on the same file layer. `game_config_common_fields`
reads the editable config files (up to 40) and matches key and section names
case-insensitively against a built-in catalog: language
(`Language`, `Locale`, `Culture`, `LanguageID`, `TextLanguage`, …) and player
name (`UserName`, `Username`, `PlayerName`, `Nickname`, `AccountName`, …).
Section-derived matches (`[Internationalization] Fallback`, `[Player] Name`) and
Unity's `_h<hash>` suffix are handled; only string (and numeric language ids,
reported as unmappable) values qualify. Each match carries a confidence and a
source: `certain`/`likely` resolve to one file + key, `ambiguous` lists the
candidates and resolves to nothing until the user pins one
(`save_game_config_field_mapping`). A pinned mapping always wins over the
catalog and is exported as JSON. Detection never writes.

Applying a field normalises the value first: language codes and names map onto
the style the file already uses (`en`→`fr`, `english`→`french`,
`English`→`French`, `Français`→`Deutsch`, `en-US`→`fr`) and the file's own
allowed list (GOG `langs[n]`) takes precedence; player names are trimmed,
limited to 32 characters and a conservative character set. The preview runs the
same parsing and encoding checks as the write and returns the exact diff, so a
value Windows-1252 or JSON cannot represent fails before the user confirms. The
write then goes through the normal backup + atomic replace path with
`origin: "common-field"`.

A game can opt into "keep in sync" (`set_game_config_sync_prefs`): at the next
launch, `launch` re-detects the fields and writes the mirrored app language and
profile name without a dialog, logging warnings instead of blocking the launch.
The values come from `set_config_sync_defaults` (kept current by the web app)
with the last value sent by the panel as a fallback; toggling the opt-in off
stops every write. Each applied change appears in the Backups tab with
`origin: "sync"`.

### Install lifecycle

- **Pause / resume.** Pausing stops the HTTP body read at the next chunk
  boundary, flushes and keeps the `.part` file; the job stays in the queue with
  phase `paused` (persisted across restarts). Resuming sends
  `Range: bytes=<offset>-` and continues with the same job id. Per-entry stream
  installs pause too: they stop between entries, keep the staging folder and
  the checkpoint, and resume from the first missing entry. Sequential tar
  installs cannot be paused and `pause_install` returns an actionable error.
  Cancelling discards the `.part`; cancelling a per-entry stream install keeps
  the completed files when at least one exists.
- **Bandwidth limit.** `bandwidthLimitKib` caps the copy loop of archive
  downloads, repairs and stream installs. 0 means unlimited. The cap is read
  when a transfer starts.
- **Manifests.** At install completion the desktop hashes every installed file
  (relative path, size, SHA-256) into `install_manifests`. Hashing failures are
  logged and never fail the install; such installs report `noManifest` until
  reinstalled. `verify_install` compares sizes first and hashes only matching
  files; extras are reported but do not count as damage. `repair_install`
  downloads the version archive (Range resume, or the local library file),
  extracts it to a temporary folder and copies back only the missing/corrupt
  files after checking their hashes. When no archive can be obtained it returns
  `fallback: true` with a message asking for a full reinstall. An adopted folder
  (#129) has no manifest until `build_install_manifest` hashes it; until then
  verify reports `noManifest` instead of a clean pass.
- **Game updates (#30).** `check_game_updates` compares each installed game with
  `GET /api/games/{id}/versions/latest`: a different `versionId` is a
  `newVersion`, an identical id with a different `archiveHash` is a
  `replacedArchive`. `update_game` reuses the install pipeline into staging; the
  previous folder is untouched until the new install is in place and indexed,
  then removed. Saves live outside the install folder and are never modified.
- **Playtime sessions (#26).** A session is opened at launch and closed when the
  whole process tree is empty (polled once per second), not only when the direct
  child exits. `sessions` stores `started_at`, `ended_at`, `minutes` (rounded,
  for the server) and `seconds` (exact, for local reports). `get_game_playtime`
  aggregates totals, per-UTC-day buckets and the recent sessions.
- **Tray (#15).** `closeToTray` (default true) hides the window instead of
  quitting; the tray menu offers Show/Hide, Downloads, Recent games (latest
  launch, else import/install date), Check for updates and Quit. Left click
  toggles the window and the tooltip reports active downloads.
- **App updates (#16, #163).** `stable` follows the latest GitHub release,
  `early` follows the moving `edge` prerelease. Checks run at startup at most
  once per 24 h plus on demand from `/updates`, Settings or the tray. The update
  center shows the current version, the queried endpoint, the last check, the
  available version with sanitised release notes/date and a release-page link;
  `UpdateStatus` carries `endpoint`, `skippedVersion` and `notify`. Skipping a
  version persists `skippedUpdateVersion` and silences the launch prompt, badge
  and toast only: the install stays available. Switching channel persists
  `updateChannel` and re-checks immediately. Install requires a configured
  signing key; without one the status reports `configured: false` and
  `install_update` refuses explicitly. Signature, disk and network failures are
  mapped to specific messages. No token, signing key or password is logged.
  The browser build has no updater: `/updates` explains that updates are
  desktop-only and links to the release page.
- **Move.** `move_install` takes a new install root and keeps the
  `{slug}/{versionLabel}` layout. Same filesystem: an atomic rename. Different
  filesystem: copy to a hidden sibling, verify the byte count, rename into
  place, then delete the old folder; any failure leaves the original intact.
  It refuses while the game runs or an install for it is active, and updates
  the index, the executable path and the manifest path.

### Streaming and multiple servers (#97, #98, #24)

**Sunshine (#97).** Detection reads `apps.json` and `sunshine.conf` from the
usual folders: `%ProgramFiles%\Sunshine\config`, `%ProgramFiles(x86)%`,
`%LOCALAPPDATA%\Sunshine`, `~/.config/sunshine`, the Sunshine Flatpak path and
`/etc/sunshine`; `sunshineAppsPath`/`sunshineConfPath` override them. Publishing
adds one entry per installed game (installed through Ludex or imported from
Steam):

```json
{
  "name": "Celeste",
  "cmd": "steam://rungameid/504230" | "C:\\Games\\Celeste\\Celeste.exe" | "ludex://play/42",
  "working-dir": "C:\\Games\\Celeste",
  "image-path": "C:\\Users\\me\\AppData\\Roaming\\cloud.cleeryy.ludex\\streaming\\art\\42.jpg",
  "detached": "true",
  "ludex": { "managed": true, "version": 1, "gameId": 42, "serverId": "srv_3f2a1c4b" }
}
```

The order is Steam URI, recorded executable, then a `ludex://play/<id>` URI (the
preview flags that last case because it needs a registered handler). The
`ludex` marker is what makes re-sync idempotent: only marked entries for the
current profile are updated or removed, entries written by another server stay,
user-created apps are never touched, and a second sync with unchanged input
performs no write. An empty/whitespace `apps.json` means an empty app list, a
UTF-8 BOM or UTF-16LE/BE file is decoded and written back in its original
encoding (backups stay byte-exact), and a malformed `apps.json` is reported and
left untouched. A write inside `Program Files` that Windows denies is reported
as a permission problem with the user-writable-folder/elevation hint (#150).
Covers are cached under `{appDataDir}/streaming/art/<gameId>.<ext>` (8 MiB cap,
5 s timeout); a cover failure never blocks publishing. The first write to an
existing file copies it to `apps.json.ludex-backup-<UTC>-<id>.bak`; a failed
backup aborts the write. Sunshine and Moonlight are never bundled or installed
by Ludex.

**Moonlight (#98, #151).** Installed Moonlight-qt on Windows keeps its Qt
`QSettings` in `HKCU\SOFTWARE\Moonlight Game Streaming Project`, so Ludex
reads that key read-only with `reg query` and maps `hosts\<n>\address`,
`customname`, `srvcert` and `pairstatus` to `MoonlightHost`; the registry is
never written and an absent key is not an error. Portable builds are read from
`Moonlight.ini` next to `Moonlight.exe` (then the current directory), and Linux
/ Flatpak keep `~/.config/Moonlight Game Streaming Project/Moonlight.conf` and
its Flatpak path. `.conf` parsing accepts `host\address`, `host/address` and
per-host sections; `srvcert`/`servercert` or `pairstatus` mark a host as
paired, and `manualaddress` wins over `address`, then local/remote. The source is
reported as `configSource`, and “not found”, “exists but unreadable” and
“configured but no host saved” are distinct messages. `moonlightHost` is a
manual name/IP fallback when no saved host is readable (saved hosts win).
`stream_game` resolves the app name from the Sunshine marker and runs
`moonlight stream <host> <app>` detached. A missing binary, an unreadable
config, a missing host or an unpaired host is an explicit error; macOS host
parsing is reported as unsupported (Moonlight uses a plist there) instead of
guessing.

**Servers (#24).** `servers` holds metadata only; tokens live in the OS
keychain (`secret-tool` on Linux, `security` on macOS) or, when no keychain is
available, in `{appDataDir}/secrets.db` (mode 0600) with a warning in the UI.
The active connection token still lives in `settings.json` (0600) because every
command reads it synchronously. The existing single-server install becomes the
first profile on launch; the legacy database is copied (not moved) to
`{appDataDir}/servers/<id>/index.db`, so each server has its own installed
games, sessions and install jobs. Switching is refused while a game or an
install is active. In the browser, profiles and per-server tokens live in
`localStorage` (`ludex.servers.v1`, `ludex.activeServer.v1`,
`ludex.token.<id>`), the library view and filters are namespaced per server, and
the existing token is migrated into the first profile. Removing a profile never
deletes its index or install folders.
### Emulator registry (#52)

Desktop-only JSON registry at `{appDataDir}/emulators.json`. Presets cover
RetroArch cores and standalone emulators; commands are user-edited and are
never bundled.

| Command | Args | Response |
|---|---|---|
| `get_rom_settings` | — | `{ registry: { emulators, defaults }, platforms }` |
| `save_rom_emulator` | `{ emulator }` | updated registry |
| `delete_rom_emulator` | `{ id }` | updated registry (built-ins are disabled, not removed) |
| `set_rom_default` | `{ platform, emulatorId \| null }` | updated registry |
| `validate_rom_command` | `{ command }` | `{ valid, error, program, args }` |
| `launch_rom_game` | `{ gameId, platform, romPath, emulatorId? }` | `{ sessionId, emulatorName, emulatorId }`; records a play session |
| `stop_rom_game` | `{ sessionId }` | stops the process tree (same session map as installed games) |

## Embedded server Rust API (used by the desktop app)

```rust
pub struct Config { /* env/CLI fields + builder setters */ }
impl Config {
    pub fn from_env() -> anyhow::Result<Self>;
    pub fn for_local(games_dir: PathBuf, data_dir: PathBuf, media_dir: PathBuf) -> Self;
}

pub struct ServerHandle {
    pub url: String,        // http://127.0.0.1:<port>
    pub token: String,      // JWT for the auto-provisioned local admin
    pub shutdown: tokio::sync::oneshot::Sender<()>,
}

pub async fn run(config: Config) -> anyhow::Result<()>;
pub async fn start(config: Config) -> anyhow::Result<ServerHandle>;
```

`start` binds, migrates, bootstraps the local admin when `config.local_mode`,
spawns the HTTP server, and returns once the listener is ready. Dropping/sending
on `shutdown` stops it gracefully.

## Stream install (ZIP, 7z and RAR)

Stream installs let the desktop install a remote archive without storing it:
files are written straight into the staging folder. There are two protocols.

**Sequential tar (compatibility).**
`GET /api/games/{id}/versions/{versionId}/stream` returns an uncompressed tar
stream of the archive entries. It exists so the desktop can install a remote
archive without storing it: the client writes extracted files directly from the
tar stream. ZIP, 7z and RAR (RAR4/RAR5) are supported; unsupported files return
415 and the client falls back to the normal download+extract path. Encrypted
archives return 409 `archive_password_required` because the stream route has no
password parameter. `Content-Length` is set to the exact tar size so the desktop
can show a percentage.

**Per-entry (resumable, ZIP only, #115).**
`GET …/stream/manifest` lists the ZIP central directory in order. Each entry has
a stable `index` (its central-directory position), a sanitized relative `path`,
uncompressed/compressed sizes, permission bits, CRC-32 and `isDir`.
`GET …/stream/entries/{index}` serves the raw entry with single-Range support:
stored entries seek directly to their data offset; DEFLATE/Zstd entries are
decompressed from the start and the offset is skipped. Directories answer 404;
out-of-range indexes answer 404 `entry_not_found`; unsatisfiable or malformed
ranges answer 416 with `Content-Range: bytes */<size>`. Path validation is
identical to the tar stream: absolute paths, `..`, drive letters, NUL bytes and
symlinks are rejected.

The desktop persists the manifest and a completed-entry bitset in its SQLite
index. On retry, pause/resume or app restart it verifies the manifest
fingerprint (SHA-256 of the JSON), keeps completed files, resumes a partial
entry with `Range` and requests only missing entries. Cancel stops between
entries and keeps completed files when at least one exists. 7z/RAR archives and
servers without these endpoints fall back to the tar stream, then to the normal
resumable download. No archive is written to the download folder on either
stream path.

Core API:

```rust
pub struct StreamEntry { pub index: usize, pub path: String, pub size: u64,
    pub compressed_size: u64, pub mode: u32, pub crc32: u32, pub is_dir: bool }
pub struct StreamManifest { pub format: String, pub entry_count: usize,
    pub total_bytes: u64, pub total_compressed_bytes: u64, pub entries: Vec<StreamEntry> }
pub fn zip_stream_manifest(path: &Path) -> Result<StreamManifest, ArchiveError>;
pub fn zip_entry_info(path: &Path, index: usize) -> Result<StreamEntry, ArchiveError>;
pub fn write_zip_entry_range<W: std::io::Write>(
    path: &Path, index: usize, offset: u64, length: Option<u64>, writer: &mut W,
) -> Result<u64, ArchiveError>;
pub fn zip_tar_size(path: &Path) -> Result<u64, ArchiveError>;
pub fn stream_zip_as_tar<W: std::io::Write>(
    path: &Path, writer: &mut W,
) -> Result<TarStreamReport, ArchiveError>;
pub fn archive_tar_size(path: &Path) -> Result<u64, ArchiveError>;
pub fn stream_archive_as_tar<W: std::io::Write>(
    path: &Path, writer: &mut W,
) -> Result<TarStreamReport, ArchiveError>;
pub struct TarStreamReport { pub files: u64, pub bytes: u64, pub tar_bytes: u64 }
```

Tar entries must use sanitized relative paths, reject traversal/symlinks/special
files, keep file modes when available, and never load a whole entry into memory.
The byte count returned by `archive_tar_size` comes from the same entry plan as
the stream writer, and the stream re-checks it before completing. RAR streaming
decompresses entries through libunrar into a private scratch file (libunrar can
only extract to a path), copied to the tar in 64 KiB chunks and deleted
immediately; the scratch directory never outlives the request.
Per-entry entries follow the same rules: sanitised relative paths, no links or
special files, file modes preserved and chunked copies.

## Shared Rust core API

`ludex-core` exposes (server + desktop depend on it):

```rust
pub fn parse_filename(name: &str) -> ParsedName;
pub fn detect_format(path: &Path) -> io::Result<ArchiveFormat>;
pub fn verify_archive(path: &Path, password: Option<&str>) -> Result<ArchiveReport>;
pub fn extract_archive(path: &Path, dest: &Path, opts: &ExtractOptions,
                       progress: &mut dyn FnMut(ExtractProgress)) -> Result<ExtractReport>;
pub fn pick_executable(root: &Path, title: &str) -> Option<PathBuf>;
pub fn discover_executables(root: &Path, title: &str, max_depth: usize) -> Discovery;
pub fn slugify(s: &str) -> String;
```

`Discovery` is the read-only folder scan (#134): ranked `ExecutableCandidate`s
with depth/tier/score/size, the chosen executable (never an unrelated file among
several) and path-bearing notes for unreadable folders, broken links and depth
truncation. `discover_executables` follows junctions/symlinks with a visited set
and clamps `max_depth` to 4..=8.

Archive support: zip, rar (RAR4/RAR5), 7z. Extraction must be streaming, must
reject absolute paths and `..` entries, and must support a password list.

## OpenAPI document

`GET /api/openapi.json` is public and always enabled. It is generated from the
server's route table and `utoipa` DTO schemas; the contract test
(`crates/ludex-server/tests/openapi.rs`) fails when a route registration and the
document disagree. `GET /api/docs` renders a minimal explorer when
`LUDEX_OPENAPI_DOCS=true` (404 otherwise). See `docs/OPENAPI.md`.

## Sessions and revocation (#19)

Every password/OIDC login issues a JWT with a `jti` backed by an
`auth_sessions` row.

```ts
interface AuthSession {
  id: number;
  userAgent: string | null;
  ip: string | null;
  createdAt: string;
  lastSeenAt: string;
  expiresAt: string;
  revokedAt: string | null;
  current: boolean;
}
```

- `POST /api/auth/login` → `{ token, user, expiresAt }`. Failures return 401
  `invalid_credentials`; repeated failures return 429 `login_throttled` with a
  `Retry-After` header (per IP + username, exponential backoff, then lockout).
- `POST /api/auth/logout` revokes the current token (204). Using it afterwards
  returns 401 `session_revoked`.
- `GET /api/auth/sessions` lists the caller's sessions, newest first.
- `DELETE /api/auth/sessions/{id}` revokes one session (own, or any as admin).
- `POST /api/me/password` (alias of `/api/auth/change-password`) verifies the
  current password, updates the hash and revokes every other session of the
  account. The calling session stays valid.
- `DELETE /api/users/{id}/sessions` (admin) revokes every session of an
  account and invalidates all older tokens. Returns `{ "revoked": n }`.

All 429 responses carry `Retry-After` (seconds). The error envelope is
unchanged.

## Resumable archive uploads (#21)

Admin/editor only. Chunks live under `{data}/uploads/{id}`; the response shape
is:

```ts
interface UploadSession {
  id: string; fileName: string; size: number; sha256: string | null;
  chunkSize: number; totalParts: number; receivedBytes: number;
  receivedParts: number[]; status: "open" | "complete" | "failed";
  createdAt: string; updatedAt: string;
  gameId: number | null; versionId: number | null; error: string | null;
}
```

- `POST /api/uploads` with `{ fileName, size, sha256? }` → 201. `fileName` must
  end in `.zip`, `.rar` or `.7z`; `size` is bounded by `LUDEX_UPLOAD_MAX_BYTES`.
  `sha256` is optional, lowercase hex, verified at completion.
- `PUT /api/uploads/{id}/parts/{n}` carries the raw chunk as the request body
  (`application/octet-stream`). The exact expected length is
  `min(chunkSize, size - n*chunkSize)`. Re-sending an identical chunk is
  idempotent and returns 200.
- `GET /api/uploads/{id}` reports progress for resume-after-reload.
- `POST /api/uploads/{id}/complete` assembles the parts, verifies size/SHA-256,
  writes the archive into the ingest library root (suffixing the name on
  collision), indexes it and returns
  `{ id, gameId, versionId, fileName, size, sha256 }`. Calling it again on a
  completed upload is idempotent. Errors: 400 `upload_incomplete` /
  `invalid_part_size`, 422 `size_mismatch` / `hash_mismatch`.
- `DELETE /api/uploads/{id}` abandons the upload and removes its chunks.
- Unfinished uploads older than `LUDEX_UPLOAD_TTL_HOURS` (default 24 h) are
  removed lazily on the next initiate and by an hourly worker.

## Configuration for the new features

| Variable | Default | Purpose |
| --- | --- | --- |
| `LUDEX_JWT_TTL_MINUTES` | `10080` (7 days) | Session token lifetime; sessions also expire server-side |
| `LUDEX_RATE_LIMIT_RPS` | `100` (`0` disables) | Global requests/second per client IP |
| `LUDEX_RATE_LIMIT_BURST` | `300` | Global burst per client IP |
| `LUDEX_DIAGNOSTICS_RATE_LIMIT` | `10` (`0` disables) | Anonymous diagnostics reports accepted per client IP per hour (#171) |
| `LUDEX_LOGIN_MAX_ATTEMPTS` | `5` (`0` disables) | Failed logins before lockout |
| `LUDEX_LOGIN_LOCKOUT_SECONDS` | `900` | Lockout duration |
| `LUDEX_LOGIN_BACKOFF_MS` | `500` | Base exponential backoff from the third failure |
| `LUDEX_UPLOAD_CHUNK_BYTES` | `16777216` | Chunk size returned by `POST /api/uploads` (capped at 64 MiB) |
| `LUDEX_UPLOAD_MAX_BYTES` | `107374182400` | Largest accepted archive upload |
| `LUDEX_UPLOAD_TTL_HOURS` | `24` | Stale upload cleanup age |
| `LUDEX_DESKTOP_HANDOFF_TTL_SECONDS` | `120` (30–600) | Lifetime of a pending desktop SSO handoff (#157) |
| `LUDEX_DESKTOP_RELEASES_API` | `https://api.github.com/repos/cleeryy/ludex/releases/latest` | GitHub-compatible releases endpoint polled by the public download page (#164); no token |
| `LUDEX_DESKTOP_RELEASES_PAGE` | `https://github.com/cleeryy/ludex/releases` | Releases page used as the offline fallback link (#164) |
| `LUDEX_DESKTOP_DOWNLOAD_BASE_URL` | unset | Flat mirror replacing asset URLs (`{base}/{fileName}`), e.g. an internal artifact proxy (#164) |
| `LUDEX_DESKTOP_DOWNLOAD_TTL_SECONDS` | `600` (10–86400) | In-process cache for release metadata (#164) |
| `LUDEX_DESKTOP_DOWNLOAD_TIMEOUT_SECONDS` | `5` (1–30) | Upstream timeout for release metadata and checksums (#164) |
| `LUDEX_OPENAPI_DOCS` | `false` | Serve `/api/docs` |

## Achievements (#117, #118)

Definitions are per game; `gameId: null` marks a global achievement. Unlocks
are per account and immutable (the first timestamp wins). Hidden definitions
are masked to `{ name: "Hidden achievement", description: null, iconUrl: null }`
until the requested account unlocks them. A showcase is only visible when the
target account enabled `publicProfile`; otherwise `GET
/api/users/{id}/achievements` returns 404 `profile_not_found`.

```ts
type AchievementProvider = "manual" | "game_files" | "gog_galaxy_api" | "gog_galaxy_sqlite";

interface Achievement {
  id: number; gameId: number | null; gameTitle: string | null; coverUrl: string | null;
  key: string; name: string; description: string | null; iconUrl: string | null;
  points: number; hidden: boolean; provider: AchievementProvider;
  unlocked: boolean; unlockedAt: string | null;
  progress: number;              // 0..1
  unlockPercentage: number;      // 0..100, share of activated accounts
  source: string | null;         // where the unlock came from when unlocked
}

interface GameAchievements {
  gameId: number; total: number; unlocked: number;
  points: number; unlockedPoints: number;
  completion: number;            // 0..100, one decimal
  achievements: Achievement[];
  lastScan: { scannedAt: string; providers: string[]; found: number; unlocked: number } | null;
}

interface UserAchievements {
  user: { id: number; username: string; displayName: string | null; publicProfile: boolean };
  unlocked: number; unlockedPoints: number; games: number;
  achievements: Achievement[];   // newest unlock first
}

interface AchievementScanReport {
  gameId: number; userId: number; scannedAt: string;
  found: number; unlocked: number;
  providers: { provider: string; status: "ok" | "skipped" | "error"; found: number; unlocked: number; message: string | null }[];
  warnings: string[];
}
```

| Method | Route | Auth | Notes |
|---|---|---|---|
| GET | `/api/games/{id}/achievements` | read | `GameAchievements`; 404 `game_not_found` |
| GET | `/api/me/achievements` | read | `UserAchievements` for the caller |
| GET | `/api/users/{id}/achievements` | read | Public showcase; 404 `profile_not_found` when private |
| POST | `/api/games/{id}/achievements` | admin | `{ key, name, description?, iconPath?, points?, hidden? }` → 201 `{ gameId, key }`; 409 `achievement_exists` |
| PATCH | `/api/games/{id}/achievements/{key}` | admin | partial `{ name?, description?, iconPath?, points?, hidden? }` → `{ gameId, key }`; 404 `achievement_not_found` |
| DELETE | `/api/games/{id}/achievements/{key}` | admin | 204, cascades unlocks |
| POST | `/api/games/{id}/achievements/{key}/unlock` | write | `{ userId?, source? }` → `{ unlocked: boolean, achievement }`; `unlocked` is false on a repeat; `userId` for another account is admin-only |
| POST | `/api/games/{id}/achievements/scan` | write | `{ installPath?, appId?, providers? }` → `AchievementScanReport`; 400 `invalid_install_path` / `unknown_provider`, 403 `path_not_allowed` |

Scans are idempotent and server-side: `installPath` must live under
`LUDEX_ACHIEVEMENT_SCAN_ROOTS` (default: the library roots). New unlocks emit
`achievement.unlocked` to the audit log and webhooks. See
`docs/ACHIEVEMENTS.md` for the provider formats and environment variables.

Environment: `LUDEX_ACHIEVEMENT_SCAN_INTERVAL_HOURS` (0 = off),
`LUDEX_ACHIEVEMENT_PROVIDERS`, `LUDEX_ACHIEVEMENT_SCAN_ROOTS`,
`LUDEX_ACHIEVEMENT_LOCAL_API_URLS`, `LUDEX_ACHIEVEMENT_GALAXY_DB`,
`LUDEX_ACHIEVEMENT_ALLOW_REMOTE_APIS`.



## Controller profiles (#51)

Profiles are stored per account so a player finds the same deadzone, mapping
and vibration preference on every device. `gameId: null` is the account-wide
profile; any game id is an override. The document is validated by
`ludex_core::gamepad` before it reaches SQLite.

```ts
type ControllerAction =
  | "left" | "right" | "up" | "down" | "confirm" | "back"
  | "menu" | "downloads" | "prevTab" | "nextTab";

interface ControllerProfile {
  name: string;                 // 1-64 characters
  deadzone: number;             // 0..0.95, radial
  sensitivity: number;          // 0.1..3
  curve: number;                // 0.5..4 (1 = linear)
  invertX: boolean;
  invertY: boolean;
  triggerThreshold: number;     // 0..1, applied to buttons 6/7
  vibration: boolean;
  /** `buttonN` (0..31), `axisN-` / `axisN+` (0..7) → action. */
  mappings: Record<string, ControllerAction>;
}

interface ControllerProfileEntry {
  gameId: number | null; gameTitle: string | null;
  profile: ControllerProfile; updatedAt: string;
}

interface ControllerProfilesResponse {
  global: ControllerProfileEntry | null;
  games: ControllerProfileEntry[];
  presets: { id: "xbox" | "dualsense" | "steamdeck" | "generic"; label: string; profile: ControllerProfile }[];
}
```

| Method | Route | Notes |
|---|---|---|
| GET | `/api/controller-profiles` | current account + presets |
| PUT | `/api/controller-profiles` | body `{ gameId?: number \| null, profile }`; upserts the scope; returns the entry |
| DELETE | `/api/controller-profiles/{gameId}` | `0` deletes the account profile; 204 |

Resolution is `game ?? global ?? xbox preset`. The web client applies it in
`useGamepad`; the editor supports live pad detection, per-input remapping,
JSON import/export and reset.

### Gamepad navigation diagnostics (#137)

`useGamepad` polls only while its screen is mounted and the window is visible,
re-scans on focus and on `gamepadconnected` / `gamepaddisconnected`, and reads
the pad that last produced input (sticky while it stays connected).
Open Library (or Big Picture) with `?gamepadDebug=1` in the route query — after
the hash works too, e.g. `#/library?gamepadDebug=1` — to show an overlay with
the pad id/index/mapping, raw axes and pressed buttons, the resolved profile
and the last action, and to enable `[gamepad]` lines in `console.debug`. The
overlay is absent unless the flag is present. While a modal dialog is open the
gamepad cannot move focus behind it; B dismisses it (the same path as Escape),
and focus returns to the gamepad target afterwards.

Status: the target is Windows/WebView2 with the standard mapping; a pad that
only reports itself after its first button press is expected and handled by the
poll loop. A physical Xbox and DualSense pass on Windows is still required
before calling this verified. Linux and Steam Deck are **not** verified: Steam
Input may hide the physical pad or present a virtual Xbox pad, and the browser
Gamepad API is only available on secure origins (`tauri://localhost` and
localhost included). Use the debug overlay there to confirm the axes/buttons
before changing a profile.

### Big Picture layouts (#141)

`/bigpicture` renders one of four layouts — `shelf` (default), `hero`, `grid`,
`compact` — from a single navigation model. Directions are resolved per layout
by a pure table: the rails (`shelf`, `hero`) walk left/right and change shelf
with up/down; `grid` walks left/right and steps a row with up/down, using the
same column count as its CSS grid; `compact` walks the list with up/down and
changes shelf with left/right. A/Enter, B/Escape, Y/downloads, X/layout and
LB/RB mean the same thing in all four.

The layout is a **device** preference, not an account setting: it is stored in
`localStorage` under `ludex.bigPicture.layout` (scoped per server through
`scopedStorageKey`, like `ludex.libraryView`) and shared by a module store, so
the header control, the quick switcher (`L` on a keyboard, X on a controller)
and Settings → Appearance all apply it to a running screen without a reload.
Unknown or missing values fall back to `shelf`. Switching layouts preserves the
selected game and shelf; the ring is restored to that game.

## Emulators and ROMs (#52)

A library root has `kind: "game" | "rom"`. ROM roots index emulator ROMs
(`nes`, `sfc`, `gba`, `n64`, `cue`, `chd`, `iso`, `gdi`, `wbfs`, `nsp`, …),
group region/revision variants, attach a `platform`, read an optional local
`gamelist.xml` and never compute PC-game metadata for the entry.

```ts
interface Emulator {
  id: number; name: string; command: string; platforms: string[];
  enabled: boolean; createdAt: string; updatedAt: string;
}

interface PlatformInfo {
  id: string; label: string; extensions: string[];
  retroarchCore: string | null; gameCount: number;
}

interface EmulatorLaunch {
  gameId: number; platform: string; emulatorId: number; emulatorName: string;
  program: string; args: string[]; romPath: string; // server-side path
}
```

| Method | Route | Notes |
|---|---|---|
| GET | `/api/platforms` | platform catalog with ROM counts |
| GET | `/api/emulators` | configured emulators |
| POST | `/api/emulators` | admin; body `{ name, command, platforms[], enabled? }` |
| PATCH | `/api/emulators/{id}` | admin; partial update |
| DELETE | `/api/emulators/{id}` | admin; 204 |
| POST | `/api/emulators/validate` | admin; body `{ command }` → `{ valid, error, program, args }`; never executes |
| GET | `/api/emulators/defaults` | `[{ platform, emulatorId }]` |
| PUT | `/api/emulators/defaults` | admin; body `{ defaults: [{ platform, emulatorId \| null }] }` |
| PUT | `/api/games/{id}/emulator` | editor; body `{ emulatorId: number \| null }`; 204 |
| POST | `/api/games/{id}/launch-command` | resolve the per-game emulator and substitute `%ROM%`; 404 `emulator_not_configured`, 422 `not_a_rom` |

Commands are tokenised into `program` + `args` and spawned without a shell:
`%ROM%` is required, `%ROM_DIR%`, `%ROM_NAME%` and `%ROM_STEM%` are optional,
quotes are preserved, and `;`, `|`, `&&`, `$(`, backticks, `>` and `<` are
rejected instead of interpreted. No emulator binary is bundled.

## Web client preferences (#23, #29)

These are browser-local values, not server configuration; no API or database
shape changes.

| Storage | Key | Values |
|---|---|---|
| `localStorage` | `ludex.theme` | `v2` JSON: `config` (`selection`, `accent`, `tint`), resolved `mode`, `bases`, `vars` and `themeColor` per mode. Legacy `dark`/`light`/`system` strings are migrated to `midnight`/`light`/`system` |
| `localStorage` | `ludex.theme.custom.v1` | imported theme files, validated with the same token/contrast rules as the picker |
| `localStorage` | `ludex.locale` | a code from `apps/web/src/locales/manifest.json` (default `en`) |
| `localStorage` | `ludex.pwa.installDismissedAt` | epoch ms; the install prompt is hidden for 30 days |

The web client applies `theme` and `locale` before the first paint
(`apps/web/index.html`). The boot script replays the ready-made CSS variable
map stored in `ludex.theme`, so accents, tints and imported themes paint
without a flash and without duplicating the theme registry in HTML. It also
sets `<html lang>` and `<html dir>` from the locale manifest. The app uses
`Intl` for dates, numbers, relative times and plural categories. The service worker only caches the app shell and the public
`/api/media/` image route; every other `/api/` request, including auth and JSON,
bypasses the cache. See `docs/I18N.md`.
