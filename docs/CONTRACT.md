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
| GET | `/api/share/{token}` | public share page (`ShareView`); see *Share links (#9)* |
| POST | `/api/share/{token}/access` | exchange a share link for a 15-minute download ticket; see *Share links (#9)* |

```ts
interface HealthDetails {
  status: "ok" | "degraded";       // degraded when SELECT 1 fails
  version: string;
  commit: string;                  // LUDEX_GIT_COMMIT, or "unknown"
  uptimeSeconds: number;
  database: { ok: boolean; sizeBytes: number };
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

## Auth

| Method | Route | Notes |
|---|---|---|
| POST | `/api/auth/login` | `{ username, password }` → `{ token, user }` |
| GET | `/api/auth/me` | `{ user }` |
| POST | `/api/auth/logout` | 204 (client discards token) |
| POST | `/api/auth/change-password` | `{ currentPassword, newPassword }` |
| PUT | `/api/auth/onboarding` | `{ completed?, step? }` → `{ user }`; self-service wizard preference, see *Onboarding (#119)* |

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
| GET | `/api/games` | query: `q`, `genre`, `state`, `bookmarked`, `earlyAccess`, `yearFrom`, `yearTo`, `players`, `playtimeMin`, `playtimeMax`, `minRating`, `provider`, `sort` (`title\|releaseDate\|rating\|addedAt\|lastPlayed\|playtime`), `order` (`asc\|desc`), `page` (1), `limit` (48, max 200) → `{ items, total, page, limit }` |
| GET | `/api/games/suggest` | query: `q`, `limit` (8, max 20) → `{ query, suggestions: [{ id, title, year, coverUrl, matchedField, matchedTitle, score }] }`; top matches, best first |
| GET | `/api/games/{id}` | `GameDetail` |
| GET | `/api/games/random` | same filters as list → `GameSummary`; 404 when empty |
| PATCH | `/api/games/{id}` | editor; editable metadata fields (`altTitles`, `players`) + `launch` object; returns `GameDetail` |
| POST | `/api/games/{id}/refresh-metadata` | editor; 202 `{ queued: true }` |
| POST | `/api/games/{id}/download-ticket` | `{ versionId? }` → `{ url, expiresAt }` (15 min, no auth needed on the URL) |
| GET | `/api/games/{id}/download?versionId=&token=` | file stream, Range supported, `Content-Disposition` |
| GET | `/api/games/{id}/versions/{versionId}/stream?token=` | Stream install: an uncompressed `application/x-tar` of the archive entries, no archive stored on the client. ZIP, 7z and RAR (RAR4/RAR5) are supported; unsupported files return 415 and the client falls back, encrypted archives return 409. Exact `Content-Length`. Same auth rules as download. |
| GET | `/api/games/{id}/versions/{versionId}/stream/manifest?token=` | Ordered ZIP manifest for resumable per-entry installs (#115): `{ format: "zip", entryCount, totalBytes, totalCompressedBytes, entries: [{ index, path, size, compressedSize, mode, crc32, isDir }] }`. Same auth as `/stream` (Bearer or download ticket). Non-ZIP → `415 stream_unsupported`. |
| GET | `/api/games/{id}/versions/{versionId}/stream/entries/{index}?token=` | Raw content of one ZIP entry, single HTTP `Range` supported (`206` + `Content-Range`, `416` + `Content-Range: bytes */size`). `Content-Length` is exact. Unknown index → `404 entry_not_found`; directories → `404 entry_not_found`. Same auth as `/stream`. |
| GET | `/api/games/{id}/versions/latest` | newest non-missing version → `LatestVersion`; 404 `no_versions` when the game has none |
| GET | `/api/games/{id}/versions/{versionId}/local-path` | embedded/local mode + loopback only → `{ path }`, else 404 |

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
`game.ingested|deleted`, `download.completed`, `settings.updated`,
`settings.oidc_updated`, `webhook.created|updated|deleted`,
`save.uploaded|deleted`, `oidc.login`, `oidc.linked`.

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

## Library

| Method | Route | Notes |
|---|---|---|
| POST | `/api/library/scan` | admin/editor; 202 `{ started: boolean }`; scans every enabled root |
| GET | `/api/library/scan` | `ScanStatus` |
| GET | `/api/library/folders` | `{ gamesDir, mediaDir, dataDir }` (admin) |
| GET | `/api/library/roots` | admin + `admin` scope; `LibraryRoot[]`, ordered |
| POST | `/api/library/roots` | admin; `{ path, label?, enabled? }` → 201 `LibraryRoot`; 409 `root_exists` |
| PATCH | `/api/library/roots/{id}` | admin; `{ label?, path?, enabled?, position? }` → `LibraryRoot` |
| DELETE | `/api/library/roots/{id}?force=` | admin; 204; 409 `root_not_empty` unless `force=true` |
| POST | `/api/library/roots/{id}/scan` | admin/editor; 202 `{ started, rootId }`; scans that root only |
| GET | `/api/library/watcher` | admin; `LibraryWatcherStatus` |

```ts
interface LibraryRoot {
  id: number; label: string; path: string;
  enabled: boolean; available: boolean; position: number;
  createdAt: string; lastScanAt: string | null; lastError: string | null;
  versionCount: number;
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
  installDir: string;           // where games are installed
  downloadDir: string;          // temp archives; empty = {installDir}/.ludex-tmp
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
  discordPresence: boolean;          // Discord Rich Presence (#12), default false
  discordClientId: string;           // Discord application id, not a bot token
  installerSilentArgs: string[][];   // silent install attempts (#7), empty = presets
  installerTimeoutSecs: number;      // per attempt, default 600, clamped 30-7200
  steamRoot: string;                 // Steam folder override (#13), empty = detect
  compatUmuPath: string;             // umu-run override (#90), empty = detect
  compatProtonPath: string;          // Proton folder/script override (#90), empty = detect
  compatWinePath: string;            // wine override (#90), empty = detect
  defaultLaunchMode: "auto" | "native" | "proton" | "umu"; // new games on Linux (#89)
  cloudAutoSync: boolean;            // global automatic save backup (#108), default true
  cloudDeviceId: string;             // random UUID generated on first use, not a hardware id
  cloudDeviceLabel: string | null;   // null = host name, then "Desktop"
}

interface InstalledGame {
  gameId: number; versionId: number; title: string; version: string | null;
  installPath: string; executable: string | null; sizeBytes: number;
  installedAt: string; lastLaunchedAt: string | null;
  /** Fingerprint of the archive this install came from, when known. */
  archiveHash: string | null;
  /** `steam` entries were imported from a Steam library (#13). */
  source: "ludex" | "steam";
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
  skipped: { appId: number; title: string; reason: string }[];
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
  available: { version: string; currentVersion: string; notes: string | null;
               publishedAt: string | null } | null;
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
  deviceId: string | null; message: string | null;
}

interface CloudSaveRestoreOutcome {
  restored: boolean; strategy: CloudSaveStrategy;
  snapshotId: number | null; backupPath: string | null; message: string;
}

interface CloudSyncToggleResponse {
  gameId: number | null; globalEnabled: boolean;
  effective: { enabled: boolean; reason: CloudSaveReason };
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
| `install_game` | `{ gameId, versionId, serverUrl, token, title, version?, executable?, installDir?, downloadDir?, stream? }` | `InstallJob` (starts background; survives dialog close) |
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
| `check_for_updates` | `{ manual? }` | `UpdateStatus`; manual checks bypass the 24 h throttle |
| `install_update` | — | `UpdateStatus` after download+install; refuses when no signing key is configured |
| `restart_app` | — | `null` (restarts Ludex; Linux/macOS after an update) |
| `installer_plan` | `{ gameId?, executable? }` | `InstallerPlan`; never runs anything. On Linux `compatTool`/`prefix` show the tool a run would use |
| `run_installer` | `{ gameId, executable?, interactive?, args? }` | `InstallerReport`; Windows runs `setup.exe` directly, Linux through UMU/Proton/Wine (#93); interactive runs use no arguments and no timeout |
| `cancel_installer` | `{ gameId }` | `null`; stops a running installer at the next poll |
| `set_executable` | `{ gameId, executable }` | `InstalledGame`; the file must be inside the install folder or its Wine prefix |
| `choose_executable` | `{ gameId }` | `InstalledGame \| null`; system file dialog, records the pick |
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
| `detect_save_paths` | `{ gameId }` | `SavePaths`; runs Ludusavi CLI/manifest detection, merges persisted manual paths and saves the mapping + preview. `saves` entries are `{ path, kind: "file" \| "directory", source: "manifest" \| "manual" \| "override", primary, sizeBytes, lastModified, notes }`; `detected: false` plus `message` is the explicit "no save folder detected" state. Paths stay local |
| `redetect_save_paths` | `{ gameId }` | `SavePaths`; same as `detect_save_paths`, for a re-run after the game created its saves |
| `get_save_paths` | `{ gameId }` | `SavePaths` from SQLite + last preview; never rescans the disk |
| `set_save_paths` | `{ gameId, paths: { path, kind?, primary?, source? }[] }` | `SavePaths`; replaces the per-game manual override list, checks each path is readable and inside an allowlisted root, allows at most one primary. `source` is `manual` (default) or `override` |
| `cloud_save_status` | `{ gameId }` | `CloudSaveStatus`; detected paths, latest snapshot (remote failures go in `message`), last upload/restore, effective toggle + reason, pending retry and conflict (#108) |
| `set_cloud_save_sync` | `{ gameId?, enabled }` | `CloudSyncToggleResponse`; `gameId` null changes the global default, a number stores a per-game override. Local only |
| `sync_cloud_save_now` | `{ gameId }` | `CloudSaveStatus`; archives and uploads immediately. Works with automatic sync off; an unchanged hash is a no-op. Failures stay in `pending` |
| `restore_cloud_save` | `{ gameId, strategy: "remote" \| "keep_local" }` | `CloudSaveRestoreOutcome`; verifies the SHA-256, backs up newer local files before a `remote` restore and returns `backupPath`, runs at most once per snapshot. `keep_local` records and uploads the local state (#108) |
| `resolve_cloud_save_conflict` | `{ gameId, strategy }` | `CloudSaveRestoreOutcome`; same operation as `restore_cloud_save`, using the explicit strategy |

Events:

- `install://progress` → `InstallJob`
- `install://done` → `InstallJob`
- `install://error` → `{ jobId, message }`
- `install://jobs` → `InstallJob[]` (whole queue after any change)
- `install://move-progress` → `{ gameId, bytesDone, bytesTotal, message }`
- `install://verify-progress` → `{ gameId, filesDone, filesTotal, bytesDone, bytesTotal, message }`
- `install://repair-progress` → same shape plus `phase: "downloading" | "extracting" | "restoring" | "verifying"`
- `game://exited` → `{ sessionId, gameId, minutesPlayed, secondsPlayed }`
- `cloud-save://synced` → `gameId` (automatic upload completed or was unchanged)
- `cloud-save://pending` → `gameId` (automatic upload failed and was queued for retry)
- `local-server://status` → `{ running, url }`
- `ui://navigate` → `string` (tray menu asked for a route, e.g. `/downloads`)
- `ui://launch` → `number` (tray "Recent games" item; the UI calls `launch_game`)
- `update://available` → `UpdateStatus` (startup check found a version)
- `update://progress` → `{ downloaded, total, percent }`
- `update://installed` → `{ version, restarted }`
- `installer://done` → `{ gameId, success, exitCode, tool }` (#7, #93; `tool` is
  the Linux compatibility tool or `null`)
- `launch_game` returns `{ sessionId, tool }`; `tool` is the Linux
  compatibility tool name when one was used (#89)

Native install layout: `{installDir}/{slug}/{versionLabel}/game files` plus a
desktop SQLite index at `{appDataDir}/ludex-desktop.db` (installed games,
sessions with exact seconds, settings, install jobs, file manifests, archive
fingerprints, resumable stream manifests). Per-entry stream checkpoints live in
`install_stream_manifests` (one manifest per job/version) and the
`install_jobs.stream_completed` hex bitset (schema v9). Local archive extraction
uses `GET /api/games/{id}/versions/{vid}/local-path` when the embedded server is
in local mode; remote installs download with Range resume and delete the temp
archive by default (`keepArchives` setting false).

### Desktop integrations (#7, #12, #13, #14)

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
- **Discord presence.** Optional, off by default, uses the local Discord IPC
  (application id, no bot token). Payload: game title, public cover URL
  (`https`, non-private host), session start; idle when nothing runs.
  Reconnects with a 4 s → 120 s backoff. `presence_status` exposes
  `connected`, `lastError` and `retryInSeconds`.
- **Steam import.** Reads `steamapps/libraryfolders.vdf` and
  `appmanifest_*.acf`, keeps fully installed games whose folder exists, matches
  normalised titles (then edition-stripped titles) against the server library and
  imports selected games with `launchUri: steam://rungameid/<appid>`,
  `versionId: 0`, `source: "steam"`. Matched games keep their server `gameId`;
  unmatched ones get `-appId`. Uninstalling an import only removes the index
  entry. Playtime for negative ids is not synced to the server.
- **Playnite.** The plugin lives in `integrations/playnite/` and is a pure API
  client; there is no dedicated endpoint.

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
  `fallback: true` with a message asking for a full reinstall.
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
- **App updates (#16).** `stable` follows the latest GitHub release, `early`
  follows the moving `edge` prerelease. Checks run at startup at most once per
  24 h plus on demand. Install requires a configured signing key; without one the
  status reports `configured: false` and `install_update` refuses explicitly.
- **Move.** `move_install` takes a new install root and keeps the
  `{slug}/{versionLabel}` layout. Same filesystem: an atomic rename. Different
  filesystem: copy to a hidden sibling, verify the byte count, rename into
  place, then delete the old folder; any failure leaves the original intact.
  It refuses while the game runs or an install for it is active, and updates
  the index, the executable path and the manifest path.

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
pub fn slugify(s: &str) -> String;
```

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
| `LUDEX_LOGIN_MAX_ATTEMPTS` | `5` (`0` disables) | Failed logins before lockout |
| `LUDEX_LOGIN_LOCKOUT_SECONDS` | `900` | Lockout duration |
| `LUDEX_LOGIN_BACKOFF_MS` | `500` | Base exponential backoff from the third failure |
| `LUDEX_UPLOAD_CHUNK_BYTES` | `16777216` | Chunk size returned by `POST /api/uploads` (capped at 64 MiB) |
| `LUDEX_UPLOAD_MAX_BYTES` | `107374182400` | Largest accepted archive upload |
| `LUDEX_UPLOAD_TTL_HOURS` | `24` | Stale upload cleanup age |
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

## Web client preferences (#23, #29)

These are browser-local values, not server configuration; no API or database
shape changes.

| Storage | Key | Values |
|---|---|---|
| `localStorage` | `ludex.theme` | `dark`, `light`, `system` (default `system`) |
| `localStorage` | `ludex.locale` | a code from `apps/web/src/locales/manifest.json` (default `en`) |
| `localStorage` | `ludex.pwa.installDismissedAt` | epoch ms; the install prompt is hidden for 30 days |

The web client applies `theme` and `locale` before the first paint
(`apps/web/index.html`), sets `<html lang>` and `<html dir>` from the locale
manifest, and uses `Intl` for dates, numbers, relative times and plural
categories. The service worker only caches the app shell and the public
`/api/media/` image route; every other `/api/` request, including auth and JSON,
bypasses the cache. See `docs/I18N.md`.
