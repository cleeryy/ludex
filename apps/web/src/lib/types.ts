/**
 * Types mirror docs/CONTRACT.md exactly. Keep this file in sync with the
 * contract; everything else in the app imports from here.
 */

export type Role = "admin" | "editor" | "user";

export type ProgressState = "unplayed" | "playing" | "completed" | "abandoned" | "onHold" | "planToPlay";

export type UserSource = "local" | "oidc";

export interface User {
  id: number;
  username: string;
  email: string | null;
  role: Role;
  activated: boolean;
  avatarUrl: string | null;
  createdAt: string;
  /** Where the account identity comes from; `oidc` accounts are read-only for identity fields. */
  source: UserSource;
  /** Identity provider that owns an `oidc` account, when known. */
  oidcIssuer: string | null;
  /** Maximum age rating this account may access; `null` = unrestricted (#39). */
  ageLimit: number | null;
  /** When the account finished or dismissed the first-run wizard (#119). */
  onboardingCompletedAt: string | null;
  /** Canonical wizard step this account resumes at (#119), 0..=20. */
  onboardingStep: number;
}

export interface GameSummary {
  id: number;
  title: string;
  sortTitle: string;
  releaseDate: string | null;
  coverUrl: string | null;
  backgroundUrl: string | null;
  rating: number | null;
  genres: string[];
  earlyAccess: boolean;
  versionCount: number;
  totalSize: number;
  state: ProgressState;
  minutesPlayed: number;
  bookmarked: boolean;
  lastPlayedAt: string | null;
  /** Highest supported player count, when known. */
  maxPlayers: number | null;
  /** Field that matched a search query. */
  matchedField?: "title" | "sortTitle" | "altTitle" | null;
  /** Title or alias text that matched the query. */
  matchedTitle?: string | null;
  /** Relevance score when the row came from a search. */
  matchScore?: number | null;
  /** Entry kind: `game` for an archive, `rom` for an emulator ROM (#52). */
  kind?: "game" | "rom";
  /** Platform slug for ROM entries (`snes`, `gba`, …). */
  platform?: string | null;
  /** Per-game emulator override; null uses the platform default. */
  emulatorId?: number | null;
}

export interface GameVersion {
  id: number;
  fileName: string;
  size: number;
  version: string | null;
  releaseDate: string | null;
  earlyAccess: boolean;
  /** Sampled archive fingerprint (`sha256-sample:`), null until computed. */
  archiveHash?: string | null;
  /** Last time the version row changed. */
  updatedAt?: string | null;
  downloadUrl: string;
}

/** One archive in the editor version manager (#168). */
export interface GameVersionInfo extends GameVersion {
  /** First scan that indexed this row. */
  indexedAt: string | null;
  /** The archive vanished from its root; the row is kept for history. */
  missing: boolean;
  /** The library root holding the archive is offline. */
  rootUnavailable: boolean;
}

export interface GameVersionsResponse {
  gameId: number;
  /** Pinned version for downloads/updates; null = newest available. */
  defaultVersionId: number | null;
  /** Every row, newest first, including missing archives. */
  versions: GameVersionInfo[];
}

export interface DeleteVersionResponse {
  versionId: number;
  /** Bytes freed on disk and in object storage. */
  freedBytes: number;
  /** Pin after the deletion; null when the game has none. */
  defaultVersionId: number | null;
}

export interface GameLaunchOptions {
  executable: string | null;
  parameters: string | null;
  installerExecutable: string | null;
  installerParameters: string | null;
  uninstallerExecutable: string | null;
  uninstallerParameters: string | null;
}

export interface GameVideo {
  title: string;
  url: string;
  thumbnailUrl: string | null;
  /** Provider video id (YouTube id for IGDB trailers). */
  videoId?: string | null;
  /** `youtube` or `file`. */
  provider?: string | null;
}

export interface GameDetail extends GameSummary {
  description: string | null;
  screenshots: string[];
  /** Deprecated alias of `videos`, kept for older clients. */
  trailers: GameVideo[];
  /** Trailers and other videos, with provider metadata. */
  videos: GameVideo[];
  developers: string[];
  publishers: string[];
  tags: string[];
  ageRating: number | null;
  provider: { slug: string; id: string; url: string | null } | null;
  /** Provider aliases used by fuzzy search. */
  altTitles: string[];
  versions: GameVersion[];
  /** Pinned version for downloads, installs and update checks (#168). */
  defaultVersionId: number | null;
  launch: GameLaunchOptions;
}

export interface SaveSnapshot {
  id: number;
  gameId: number;
  versionId: number | null;
  label: string;
  fileName: string;
  size: number;
  sha256: string;
  createdAt: string;
  /** `manual` for a user upload, `auto` for a post-session sync (#108). */
  source: "manual" | "auto";
  /** Opaque device identifier that uploaded the snapshot (#108). */
  deviceId: string | null;
}

/** Effective automatic cloud-save state for one game (#108). */
export type CloudSaveReason = "on" | "global_off" | "game_off";

export interface CloudSaveToggle {
  enabled: boolean;
  reason: CloudSaveReason;
}

export interface CloudSavePath {
  path: string;
  kind: "file" | "directory" | string;
  primary: boolean;
  exists: boolean;
  files: number;
  sizeBytes: number;
  lastModified: string | null;
}

export interface CloudSavePending {
  pending: boolean;
  attempts: number;
  nextAttemptAt: string | null;
  error: string | null;
}

export interface CloudSaveConflict {
  localNewestMtime: string | null;
  lastUploadAt: string | null;
  lastHash: string | null;
  remoteSha256: string;
  remoteCreatedAt: string;
  remoteDeviceId: string | null;
}

export interface CloudSaveStatus {
  gameId: number;
  serverConfigured: boolean;
  paths: CloudSavePath[];
  localFiles: number;
  localBytes: number;
  localNewestMtime: string | null;
  latest: SaveSnapshot | null;
  lastHash: string | null;
  lastUploadAt: string | null;
  lastRestoreAt: string | null;
  toggle: CloudSaveToggle;
  conflict: CloudSaveConflict | null;
  pending: CloudSavePending;
  deviceId: string | null;
  /** ISO timestamp of the last save detection for this game, `null` if never. */
  lastDetectionAt: string | null;
  message: string | null;
}

export interface CloudSyncToggleResponse {
  gameId: number | null;
  globalEnabled: boolean;
  effective: CloudSaveToggle;
}

export type CloudSaveStrategy = "remote" | "keep_local";

export interface CloudSaveRestoreOutcome {
  restored: boolean;
  strategy: CloudSaveStrategy;
  snapshotId: number | null;
  backupPath: string | null;
  message: string;
}

export interface NewsPost {
  id: number;
  title: string;
  body: string;
  author: string;
  createdAt: string;
  updatedAt: string;
}

export type IngestStatus = "queued" | "downloading" | "verifying" | "done" | "error" | "cancelled";

export interface IngestJob {
  id: string;
  title: string;
  url: string | null;
  status: IngestStatus;
  fileName: string | null;
  downloadedBytes: number;
  totalBytes: number | null;
  speedBps: number;
  attempt: number;
  error: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface ScanStatus {
  running: boolean;
  startedAt: string | null;
  finishedAt: string | null;
  scanned: number;
  added: number;
  updated: number;
  removed: number;
  metadataPending: number;
  errors: string[];
}

export interface LibraryFile {
  name: string;
  size: number;
  partial: boolean;
  modifiedAt: string;
}

export interface IngestStats {
  gamesDir: string;
  diskTotal: number;
  diskFree: number;
  libraryBytes: number;
  gameCount: number;
  activeJobs: number;
}

export interface ServerConfig {
  serverName: string;
  version: string;
  features: { igdb: boolean; steam: boolean; ingest: boolean; localMode: boolean; oidc: boolean };
  /** Label for the SSO button, e.g. "Authentik". Null hides the button. */
  oidcDisplayName: string | null;
  maxUploadBytes: number;
}

export interface SetupStatus {
  needsSetup: boolean;
  serverName: string;
  version: string;
}

/* ---------------------------- desktop download --------------------------- */

/** Platform values accepted by `GET /api/desktop/download` (#164). */
export type DesktopPlatform = "windows" | "linux" | "macos";

export type DesktopAssetKind = "installer" | "appimage" | "deb" | "rpm" | "dmg";

/** One installer offered by the latest stable desktop release. */
export interface DesktopAsset {
  platform: DesktopPlatform;
  kind: DesktopAssetKind;
  name: string;
  url: string;
  sizeBytes: number;
  /** Lowercase hex SHA-256 when the release publishes `SHA256SUMS`. */
  sha256: string | null;
}

/** `GET /api/desktop/download` response: links only, never a proxied binary. */
export interface DesktopDownload {
  channel: "stable";
  /** `cache` means a stale answer could not be refreshed; `fallback` is the
   * releases page with no assets (offline or rate-limited upstream). */
  source: "github" | "mirror" | "cache" | "fallback";
  stale: boolean;
  version: string | null;
  tag: string | null;
  releasedAt: string | null;
  releaseUrl: string;
  notes: string | null;
  checksumsUrl: string | null;
  /** Echo of the requested platform; null when every asset was listed. */
  platform: DesktopPlatform | null;
  recommended: DesktopAsset | null;
  assets: DesktopAsset[];
}

/* ------------------------------- OIDC admin ------------------------------ */

/** Effective OIDC settings. The client secret is never included. */
export interface OidcSettings {
  enabled: boolean;
  issuer: string | null;
  clientId: string | null;
  redirectUrl: string | null;
  publicBaseUrl: string | null;
  scopes: string;
  displayName: string;
  autoCreate: boolean;
  defaultRole: Role;
  groupsClaim: string;
  adminGroups: string[];
  /** Whether a secret is stored at rest. */
  secretConfigured: boolean;
}

export interface OidcStatus {
  ok: boolean;
  issuer: string | null;
  checkedAt: string | null;
  error: string | null;
}

export interface OidcAdminResponse {
  settings: OidcSettings;
  /** camelCase field names pinned by `LUDEX_OIDC_*`. */
  envLockedFields: string[];
  status: OidcStatus;
  linkedAccounts: number;
}

/** Write payload; `replaceSecret` is write-only and never returned. */
export interface OidcSettingsInput {
  enabled?: boolean;
  issuer?: string | null;
  clientId?: string | null;
  redirectUrl?: string | null;
  publicBaseUrl?: string | null;
  scopes?: string;
  displayName?: string;
  autoCreate?: boolean;
  defaultRole?: Role;
  groupsClaim?: string;
  adminGroups?: string[];
  replaceSecret?: string | null;
  clearSecret?: boolean;
}

export interface OidcDiscoveryInfo {
  issuer: string;
  authorizationEndpoint: string;
  tokenEndpoint: string;
  jwksUri: string;
  idTokenSigningAlgValuesSupported: string[];
  tokenEndpointAuthMethodsSupported: string[];
}

export interface OidcTestResult {
  ok: boolean;
  discovery: OidcDiscoveryInfo;
}

/* ---------------------------- metadata providers -------------------------- */

/** Canonical fields a provider mapping may target. */
export type MetadataProviderField =
  | "id"
  | "title"
  | "description"
  | "releaseDate"
  | "coverUrl"
  | "backgroundUrl"
  | "screenshots"
  | "genres"
  | "developers"
  | "publishers"
  | "tags"
  | "rating"
  | "ageRating"
  | "providerUrl";

/** A community metadata provider as returned by the admin API. */
export interface MetadataProvider {
  id: number;
  name: string;
  baseUrl: string;
  /** Whether a bearer token is stored at rest. The token itself never leaves
   * the server. */
  secretConfigured: boolean;
  /** Lower runs first; IGDB always runs before every provider. */
  priority: number;
  enabled: boolean;
  /** Canonical field -> dot path in a non-canonical response. */
  mapping: Partial<Record<MetadataProviderField, string>>;
  createdAt: string;
  updatedAt: string;
}

/** Create/update payload. `token` is write-only. */
export interface MetadataProviderInput {
  name?: string;
  baseUrl?: string;
  /** Blank or absent keeps the stored token. */
  token?: string | null;
  clearToken?: boolean;
  priority?: number;
  enabled?: boolean;
  mapping?: Partial<Record<MetadataProviderField, string>>;
}

/** One metadata candidate returned by a provider. */
export interface ProviderCandidate {
  id: string;
  title: string;
  description?: string;
  releaseDate?: string;
  coverUrl?: string;
  backgroundUrl?: string;
  screenshots?: string[];
  genres?: string[];
  developers?: string[];
  publishers?: string[];
  tags?: string[];
  rating?: number;
  ageRating?: number;
  providerUrl?: string;
}

export interface ProviderTestResult {
  ok: boolean;
  candidates: ProviderCandidate[];
  error?: string;
  statusCode?: number;
  elapsedMs: number;
}

/** One record the editor can pick in the manual match dialog (#135). */
export interface MetadataCandidate {
  provider: string;
  providerName: string;
  providerId: string;
  providerUrl: string | null;
  title: string;
  year: number | null;
  coverUrl: string | null;
  summary: string | null;
  developers: string[];
  platforms: string[];
  /** Relevance score; higher is a closer title/year match. */
  score: number;
  /** `score` normalised to 0-1. */
  confidence: number;
  /** Whether this record is the game's current match. */
  selected: boolean;
}

/** Per-provider outcome of a candidates query. */
export interface MetadataProviderStatus {
  slug: string;
  name: string;
  ok: boolean;
  error: string | null;
  candidates: number;
  elapsedMs: number;
}

export interface MetadataCandidates {
  query: string;
  gameId: number;
  /** Metadata fields edited by hand; preserved by default when applying. */
  manualFields: string[];
  candidates: MetadataCandidate[];
  providers: MetadataProviderStatus[];
}

export interface MetadataApplyInput {
  provider: string;
  providerId: string;
  replaceManual?: boolean;
}

export interface LibraryStats {
  games: number;
  versions: number;
  totalBytes: number;
  users: number;
  byState: Record<ProgressState, number>;
}

export interface AuthResult {
  token: string;
  user: User;
  /** Token expiry, when the server tracks the session. */
  expiresAt?: string;
}

/** `POST /api/auth/desktop-handoff` (#157). Never contains a code or a token. */
export interface DesktopHandoff {
  /** When the pending handoff stops being redeemable. */
  expiresAt: string;
}

/** `oidc_login_start` (#157): opened URL and the id used to poll/cancel. */
export interface OidcLoginStart {
  sessionId: string;
  /** Plain `{server}/#/login`, offered when the browser cannot be opened. */
  webLoginUrl: string;
  /** When the app stops waiting for the web page. */
  expiresAt: string;
}

/** `oidc_login_poll` status (#157). */
export type OidcLoginStatus = "pending" | "claimed" | "expired" | "cancelled" | "error";

/** `oidc_login_poll` result (#157). `code` is set only for `claimed`. */
export interface OidcLoginPoll {
  status: OidcLoginStatus;
  code: string | null;
  message: string | null;
}

export interface GameListQuery {
  q?: string;
  genre?: string;
  state?: ProgressState;
  bookmarked?: boolean;
  earlyAccess?: boolean;
  /** Earliest release year (inclusive). */
  yearFrom?: number;
  /** Latest release year (inclusive). */
  yearTo?: number;
  /** Minimum supported player count. */
  players?: number;
  /** Minimum played minutes (inclusive). */
  playtimeMin?: number;
  /** Maximum played minutes (inclusive). */
  playtimeMax?: number;
  /** Minimum rating (0-100). */
  minRating?: number;
  /** Metadata provider slug (`igdb`, `steam`, `demo`). */
  provider?: string;
  /** Platform slug for ROM entries (`snes`, `gba`, …). */
  platform?: string;
  /** Entry kind (`game` or `rom`). */
  kind?: "game" | "rom";
  sort?: "title" | "releaseDate" | "rating" | "addedAt" | "lastPlayed" | "playtime";
  order?: "asc" | "desc";
  page?: number;
  limit?: number;
}

/** One fuzzy-search suggestion returned by `GET /api/games/suggest`. */
export interface GameSuggestion {
  id: number;
  title: string;
  year: number | null;
  coverUrl: string | null;
  matchedField: "title" | "sortTitle" | "altTitle";
  matchedTitle: string;
  score: number;
}

export interface SuggestResponse {
  query: string;
  suggestions: GameSuggestion[];
}

export interface GameList {
  items: GameSummary[];
  total: number;
  page: number;
  limit: number;
}

export interface GamePatch {
  title?: string;
  description?: string | null;
  releaseDate?: string | null;
  rating?: number | null;
  genres?: string[];
  tags?: string[];
  developers?: string[];
  publishers?: string[];
  ageRating?: number | null;
  earlyAccess?: boolean;
  altTitles?: string[];
  players?: number | null;
  launch?: Partial<GameLaunchOptions>;
}

/* ------------------------------- community ------------------------------- */

export type CollectionKind = "manual" | "smart";

export type SmartField =
  | "genre"
  | "year"
  | "players"
  | "playtime"
  | "rating"
  | "installed"
  | "provider"
  | "title";

export type SmartOperator = "is" | "contains" | "gte" | "lte" | "between";

export interface SmartRule {
  field: SmartField;
  op?: SmartOperator;
  value: string | number | boolean;
  value2?: string | number;
}

export interface SmartRules {
  match: "all" | "any";
  rules: SmartRule[];
}

export interface CollectionOwner {
  id: number;
  username: string;
  displayName: string | null;
}

export interface Collection {
  id: number;
  name: string;
  description: string | null;
  isPublic: boolean;
  kind: CollectionKind;
  rules: SmartRules;
  gameCount: number;
  owner: CollectionOwner;
  owned: boolean;
  createdAt: string;
  updatedAt: string;
}

export interface CollectionInput {
  name: string;
  description?: string | null;
  isPublic?: boolean;
  kind?: CollectionKind;
  rules?: SmartRules;
  gameIds?: number[];
}

export interface CollectionGamesResponse {
  added: number;
  total: number;
}

export interface ActivityUser {
  id: number;
  username: string;
  displayName: string | null;
  avatarUrl: string | null;
}

export interface ActivityGame {
  id: number;
  title: string;
  coverUrl: string | null;
  releaseYear: number | null;
}

export interface ActivityCollection {
  id: number;
  name: string;
}

export type ActivityKind =
  | "game_added"
  | "game_installed"
  | "game_played"
  | "game_completed"
  | "collection_created"
  | "achievement_unlocked";

export interface Activity {
  id: number;
  kind: ActivityKind;
  createdAt: string;
  metadata: Record<string, unknown>;
  user: ActivityUser;
  game: ActivityGame | null;
  collection: ActivityCollection | null;
}

export interface ActivityPage {
  items: Activity[];
  nextBefore: number | null;
}

export interface PublicProfile {
  id: number;
  username: string;
  displayName: string | null;
  bio: string | null;
  avatarUrl: string | null;
  publicProfile: boolean;
  createdAt: string;
}

export interface PublicStats {
  games: number;
  completed: number;
  minutesPlayed: number;
  bookmarked: number;
  collections: number;
}

export interface PublicProfileResponse {
  user: PublicProfile;
  stats: PublicStats;
  library: GameSummary[];
  collections: Collection[];
}

export interface ProfilePatch {
  publicProfile?: boolean;
  displayName?: string | null;
  bio?: string | null;
}

/* ------------------------------- desktop -------------------------------- */

/** Named install/download/temp folder (#99). */
export type LocationKind = "install" | "download" | "temp";

export interface InstallLocation {
  /** Stable id passed to `install_game`; filled by the backend. */
  id: string;
  label: string;
  path: string;
  kind: LocationKind;
  /** At most one default per kind. */
  default: boolean;
}

/** Live probe for a location (#99). */
export interface LocationStatus extends InstallLocation {
  exists: boolean;
  writable: boolean;
  freeBytes: number | null;
  totalBytes: number | null;
  note: string | null;
  requiredBytes?: number | null;
  freeSpaceOk?: boolean | null;
}

/** Volume kind reported by `suggest_storage_locations` (#145). */
export type StorageVolumeKind = "fixed" | "removable" | "network" | "other";

/** One detected install-root candidate (#145). Never created by the probe. */
export interface StorageSuggestion {
  path: string;
  /** Short volume name: `C:`, `Home`, `games`. */
  label: string;
  volume: string;
  kind: StorageVolumeKind;
  exists: boolean;
  writable: boolean;
  freeBytes: number | null;
  totalBytes: number | null;
  recommended: boolean;
  current: boolean;
  fallback: boolean;
  nearlyFull: boolean;
  note: string | null;
}

/** Response of `suggest_storage_locations` (#145). */
export interface StorageSuggestions {
  suggestions: StorageSuggestion[];
  recommendedPath: string | null;
  currentPath: string;
  currentIsFallback: boolean;
  needsChoice: boolean;
  platform: "windows" | "macos" | "linux";
}

export interface LocalSettings {
  mode: "local" | "remote";
  /** Opt-in anonymous diagnostics (#171). */
  diagnosticsEnabled: boolean;
  /** Random install id used only with the diagnostics opt-in. */
  diagnosticsInstallId: string | null;
  serverUrl: string;
  token: string | null;
  gamesDir: string | null;
  /** Where games are installed. Mirrors the default install location (#99). */
  installDir: string;
  /**
   * True once the user confirmed an install root (#145). While false, a new
   * install preselects the detected-drive recommendation instead of the
   * app-data default.
   */
  installRootConfirmed: boolean;
  downloadDir: string;
  /** Named locations (#99); installDir/downloadDir mirror the defaults. */
  locations: InstallLocation[];
  serverPort: number;
  /** Remote ZIP installs stream a tar of the archive entries instead of storing it. */
  streamInstall: boolean;
  /** Keep the downloaded archive after a successful install. */
  keepArchives: boolean;
  /** Archive passwords, tried in order. */
  archivePasswords: string[];
  /** Global download cap in KiB/s. 0 means unlimited. */
  bandwidthLimitKib: number;
  /** Closing the window hides Ludex to the tray instead of quitting. */
  closeToTray: boolean;
  /** Use the OS title bar instead of Ludex's own (#111). Off by default. */
  nativeTitleBar: boolean;
  /** Release channel followed by the app updater. */
  updateChannel: "stable" | "early";
  /** Check for app updates at startup (throttled to once per 24 h). */
  checkUpdatesOnStartup: boolean;
  /** Last update check, RFC 3339. */
  lastUpdateCheckAt: string | null;
  /** Version the user skipped (#163); only silences notifications. */
  skippedUpdateVersion?: string | null;
  /** Publish the running game as Discord Rich Presence (#12). Off by default. */
  discordPresence: boolean;
  /** Discord application id used for Rich Presence (not a bot token). */
  discordClientId: string;
  /** Argument sets tried in order by the silent installer (#7). Empty = presets. */
  installerSilentArgs: string[][];
  /** Seconds one silent install attempt may run before it is killed. */
  installerTimeoutSecs: number;
  /** Steam root overriding auto-discovery (#13). Empty = detect. */
  steamRoot: string;
  /** `umu-run` override for the compatibility scan (#90). Empty = detect. */
  compatUmuPath: string;
  /** Proton folder or `proton` script override (#90). Empty = detect. */
  compatProtonPath: string;
  /** `wine` override for the compatibility scan (#90). Empty = detect. */
  compatWinePath: string;
  /** Launch mode new games start with on Linux (#89). */
  defaultLaunchMode: LaunchMode;
  /** Explicit Ludusavi manifest file (#107). Empty = auto-managed copy, then a detected install. */
  ludusaviManifestPath: string;
  /** `ludusavi` binary override (#107). Empty = look for it on PATH. */
  ludusaviBinary: string;
  /** Download/refresh the save database automatically (#153), at most once per 24 h. */
  ludusaviManifestAutoUpdate: boolean;
  /** Mirror or pinned URL for the manifest download (#153). Empty = the canonical upstream file. */
  ludusaviManifestUrl: string;
  /** Global default for automatic save backups after a session (#108). */
  cloudAutoSync: boolean;
  /** Stable opaque device id generated on first use (#108). */
  cloudDeviceId: string;
  /** Device name shown on snapshots (#108); null uses the host name. */
  cloudDeviceLabel: string | null;
  /** Saved Ludex servers (#24). Tokens live in the secret store, never here. */
  servers: ServerProfile[];
  /** Id of the profile backing `serverUrl`/`token`. */
  activeServerId: string;
  /** Sunshine `apps.json` override (#97). Empty = detect. */
  sunshineAppsPath: string;
  /** Sunshine `sunshine.conf` override (#97). Empty = next to apps.json. */
  sunshineConfPath: string;
  /** Moonlight binary override (#98). Empty = detect. */
  moonlightBinary: string;
  /** Moonlight config override (#98). Empty = detect. */
  moonlightConfigPath: string;
  /** Manual Moonlight host name or IP (#151) for builds Ludex cannot read. */
  moonlightHost: string;
  /**
   * Publish one `Ludex` console app to Sunshine instead of one entry per game
   * (#172). Off by default so an existing per-game setup is unchanged.
   */
  consoleMode: boolean;
  /** Sunshine name of the console app (#172); empty means `Ludex`. */
  consoleAppName: string;
  /** Start the console app fullscreen (default); off publishes `--windowed`. */
  consoleFullscreen: boolean;
  /** In-console chord that quits back to the desktop (#172), e.g. `f12`. */
  consoleExitShortcut: string;
  /**
   * Expert option: keep publishing one Sunshine app per game in addition to
   * the console app (#172). Ignored while `consoleMode` is off.
   */
  consolePerGameApps: boolean;
  /** Ordered GameVault install roots (#134). */
  gamevaultRoots: string[];
  /** Folder depth used by GameVault scans (#134), 4-8, default 6. */
  gamevaultScanDepth: number;
  /**
   * Launcher sources `launcher_scan` may read (#155), in display order. All
   * are enabled by default; an empty list means the user unchecked everything.
   * Disabling a source never removes games already imported from it.
   */
  launcherSources: LauncherId[];
  /** Ludex UI language mirrored from the web locale (#148), used by the launch-time config sync. */
  uiLanguage?: string;
  /** Ludex profile display name mirrored from the web profile (#148). */
  playerName?: string;
  /** In-game controller overlay (#144). On by default. */
  overlayEnabled?: boolean;
  /** Gesture recognised by the native listener (#144). */
  overlayTrigger?: OverlayTrigger;
  /** Overlay panel background opacity, 0.35-1.0 (#144). */
  overlayOpacity?: number;
  /** Overlay panel scale, 0.6-1.3 (#144). */
  overlayScale?: number;
  /** Tabs shown in the overlay (#144); the game tab is always drawn. */
  overlayTabs?: OverlayTabId[];
  /** Games where the overlay stays off (#144). Local to this device. */
  overlayDisabledGames?: number[];
}

/* --------------------- in-game overlay (#144) ---------------------------- */

/** Gesture that opens the overlay. */
export type OverlayTrigger = "doubleHome" | "chord" | "keyboard";

/** Content tab of the overlay, in canonical display order. */
export type OverlayTabId = "game" | "library" | "downloads" | "achievements";

/** One controller as reported by the native global listener. */
export interface OverlayController {
  /** Process-local id, stable while the pad stays connected. */
  id: string;
  name: string;
  /** True for the pad that produced input most recently. */
  active: boolean;
  battery: number | null;
  charging: boolean;
}

/** Native listener state shown in the overlay and in Settings. */
export interface OverlayListenerStatus {
  /** `gilrs`, `unsupported` or `failed`. */
  backend: string;
  available: boolean;
  platform: string;
  reason: string | null;
  controllers: OverlayController[];
}

/** The game currently running, as shown in the overlay. */
export interface OverlayGameState {
  sessionId: string;
  gameId: number;
  title: string;
  /** RFC 3339 launch time. */
  startedAt: string;
  /** Seconds since the launch. */
  elapsedSeconds: number;
  /** Finished playtime plus the current session, in seconds. */
  totalSeconds: number;
  /** True when the overlay is disabled for this game. */
  disabled: boolean;
}

/** Everything the overlay UI and the settings panel need. */
export interface OverlayStatus {
  enabled: boolean;
  open: boolean;
  trigger: OverlayTrigger;
  opacity: number;
  scale: number;
  tabs: OverlayTabId[];
  disabledGames: number[];
  game: OverlayGameState | null;
  gameDisabled: boolean;
  /** `window` = floating window over the game, `inApp` = inside the main window. */
  presentation: "closed" | "window" | "inApp";
  listener: OverlayListenerStatus;
  /** Platform limitation, shown verbatim in Settings. */
  platformNote: string;
  /** Shortcut that works while Ludex is focused. */
  keyboardShortcut: string;
}

/** Partial update accepted by `set_overlay_settings`. */
export interface OverlaySettingsPatch {
  enabled?: boolean;
  trigger?: OverlayTrigger;
  opacity?: number;
  scale?: number;
  tabs?: OverlayTabId[];
  disabledGames?: number[];
  /** Opt-in anonymous diagnostics and crash reports (#171). Off by default. */
  diagnosticsEnabled?: boolean;
  /** Random install id generated locally when diagnostics are enabled (#171). */
  diagnosticsInstallId?: string;
}

/* ------------------ Anonymous diagnostics and crash reports (#171) ----------------- */

/** One report stored on this device by the desktop app. */
export interface DiagnosticsReportSummary {
  id: string;
  createdAt: string;
  kind: string;
  summary: string;
  sent: boolean;
}

/** Local diagnostics state returned by the desktop commands. */
export interface DiagnosticsStatus {
  enabled: boolean;
  installId: string;
  appVersion: string;
  platform: string;
  os: string;
  locale: string;
  channel: string;
  reports: DiagnosticsReportSummary[];
  unsent: number;
}

/** Exact payload a send would use, as shown in the preview. */
export interface DiagnosticsPreview {
  reportId: string;
  payload: Record<string, unknown>;
}

/** Result of an explicit send. */
export interface DiagnosticsSendOutcome {
  reportId: string;
  sent: boolean;
}

/** One report stored by the server for admin review. */
export interface ServerDiagnosticsReport {
  id: number;
  installId: string;
  appVersion: string;
  platform: string;
  os: string;
  locale: string;
  channel: string;
  errorKind: string;
  summary: string;
  detail: Record<string, unknown>;
  createdAt: string;
}

/** One aggregate count in the admin page. */
export interface DiagnosticsCount {
  value: string;
  count: number;
}

/** Admin diagnostics page with counts and recent reports. */
export interface DiagnosticsPage {
  items: ServerDiagnosticsReport[];
  nextBefore: number | null;
  total: number;
  byVersion: DiagnosticsCount[];
  byKind: DiagnosticsCount[];
  byPlatform: DiagnosticsCount[];
  retentionDays: number;
}

/* ------------------- Ludusavi save-path manifest (#153) ------------------ */

/**
 * Cached save-path manifest. Ludex downloads the official Ludusavi asset so a
 * fresh install detects saves without installing anything. Local only: the
 * manifest, its path and this status are never sent to the server.
 */
export interface ManifestStatus {
  present: boolean;
  /** Cached file path (`{appDataDir}/manifest/…`); null when absent. */
  path: string | null;
  /** `sha256:<12 hex>` of the cached file, when known. */
  version: string | null;
  /** RFC 3339 timestamp of the last successful download. */
  updatedAt: string | null;
  /** RFC 3339 timestamp of the last check, successful or not. */
  checkedAt: string | null;
  /** Effective download URL (canonical file or the user's mirror). */
  source: string;
  sizeBytes: number;
  /** False when the user turned automatic downloads off. */
  autoUpdate: boolean;
  /** Older than the 24 h refresh interval, or never checked. */
  stale: boolean;
  /** Last download error, kept so the panel can explain a no-op check. */
  error: string | null;
  message: string;
}

/** Result of a manual "Update manifest" click. */
export interface ManifestUpdate extends ManifestStatus {
  /** True when the cached file was replaced. */
  updated: boolean;
  /** True when the server answered `304 Not Modified`. */
  notModified: boolean;
}

/* --------------------------- servers and profiles (#24) ------------------ */

export interface ServerProfile {
  id: string;
  label: string;
  url: string;
  username: string | null;
  lastUsedAt: string | null;
  createdAt: string | null;
  active: boolean;
  local: boolean;
  hasToken: boolean;
}

export interface ServerList {
  servers: ServerProfile[];
  activeId: string | null;
  tokenStorage: "keychain" | "database";
  keychain: string | null;
  warning: string | null;
  message: string | null;
}

export interface ServerTest {
  ok: boolean;
  status: number | null;
  version: string | null;
  latencyMs: number;
  message: string;
}

/* --------------------- Sunshine host and Moonlight (#97, #98) ------------ */

/** Runtime console status (#172); `active` only when `ludex --console` ran. */
export interface ConsoleStatus {
  active: boolean;
  fullscreen: boolean;
  appName: string;
  exitShortcut: string;
}

export interface SunshineStatus {
  detected: boolean;
  platform: string;
  appsPath: string | null;
  confPath: string | null;
  binary: string | null;
  source: string | null;
  overridden: boolean;
  appsFound: boolean;
  entries: number;
  managedEntries: number;
  warnings: string[];
  message: string;
}

export interface SunshinePreviewEntry {
  gameId: number;
  title: string;
  name: string;
  cmd: string;
  workingDir: string;
  imagePath: string | null;
  action: "add" | "update" | "unchanged";
  needsLudexScheme: boolean;
  /** True for the single console entry, which uses `gameId: 0` (#172). */
  console?: boolean;
}

export interface SunshineRemoval {
  name: string;
  gameId: number | null;
}

export interface SunshineSkipped {
  gameId: number;
  title: string;
  reason: string;
}

export interface SunshinePreview {
  status: SunshineStatus;
  entries: SunshinePreviewEntry[];
  removals: SunshineRemoval[];
  skipped: SunshineSkipped[];
  canSync: boolean;
  message: string;
}

export interface SunshineReport {
  written: boolean;
  dryRun: boolean;
  appsPath: string | null;
  backupPath: string | null;
  added: number;
  updated: number;
  unchanged: number;
  removed: number;
  managedEntries: number;
  skipped: SunshineSkipped[];
  message: string;
}

export interface MoonlightHost {
  name: string;
  address: string;
  localAddress: string | null;
  remoteAddress: string | null;
  manualAddress: string | null;
  paired: boolean;
}

export interface MoonlightStatus {
  detected: boolean;
  platform: string;
  binary: string | null;
  configPath: string | null;
  /** `Moonlight.ini`, `Windows registry`, `Moonlight.conf` or `override` (#151). */
  configSource: string | null;
  configFound: boolean;
  /** Manual host fallback from Settings → Streaming (#151). */
  manualHost: string | null;
  hosts: MoonlightHost[];
  supported: boolean;
  warnings: string[];
  message: string;
}

export interface StreamLaunch {
  launched: boolean;
  host: string;
  app: string;
  command: string[];
  message: string;
}

export interface StreamTarget {
  published: boolean;
  appName: string | null;
  gameId: number;
  hosts: MoonlightHost[];
  moonlight: MoonlightStatus;
  sunshine: SunshineStatus;
  message: string;
}

export interface InstalledGame {
  gameId: number;
  versionId: number;
  title: string;
  version: string | null;
  installPath: string;
  executable: string | null;
  sizeBytes: number;
  installedAt: string;
  lastLaunchedAt: string | null;
  /** Fingerprint of the archive this install came from, when known. */
  archiveHash?: string | null;
  /** `steam` entries were imported from a Steam library and launch through Steam. */
  /** `ludex` entries were installed here, `adopted` folders were linked in place (#129); everything else is a launcher import (#116). */
  source?: "ludex" | "adopted" | LauncherId;
  /** External launch URI for imported entries (`steam://rungameid/…`). */
  launchUri?: string | null;
}

/** Kind of program found in an install folder (#143). */
export type GameExecutableKind = "launch" | "installer" | "uninstaller" | "other";

/** One program inside the install folder of a game (#143). */
export interface GameExecutable {
  /** Path relative to the install folder, forward slashes, never escapes it. */
  path: string;
  name: string;
  sizeBytes: number;
  /** RFC 3339, null when the filesystem does not expose it. */
  modifiedAt: string | null;
  kind: GameExecutableKind;
  /** True for the file Ludex would launch now (`pick_executable`). */
  recommended: boolean;
}

/** Result of scanning an install folder for the Launch options pickers (#143). */
export interface GameExecutables {
  gameId: number;
  installPath: string;
  entries: GameExecutable[];
  /** Skipped links, unreadable folders, truncation. */
  notes: string[];
  scannedAt: string;
  /** True when the answer came from the short-lived desktop cache. */
  cached: boolean;
}

/** An index row that already exists for the game being adopted (#129). */
export interface AdoptExistingInstall {
  gameId: number;
  title: string;
  source: string;
  installPath: string;
}

/** Read-only preview of linking an existing folder (#129). */
export interface AdoptPreview {
  folder: string;
  executable: string | null;
  /** Ranked executables found in the folder (#134); choose one manually. */
  executableCandidates: string[];
  sizeBytes: number;
  fileCount: number;
  /** 0 means unknown; versions are never guessed from the title. */
  versionId: number;
  version: string | null;
  warnings: string[];
  existing: AdoptExistingInstall | null;
}

export interface AdoptInstallArgs {
  gameId: number;
  folder: string;
  versionId?: number | null;
  version?: string | null;
  executable?: string | null;
  /** Required to replace an existing ludex/launcher row. */
  replace?: boolean;
  title?: string | null;
}

/** Preview of deleting an adopted folder, or the result once confirmed. */
export interface DeleteAdoptedPreview {
  folder: string;
  sizeBytes: number;
  fileCount: number;
  deleted: boolean;
  /** The folder was already gone; only the index row was removed. */
  missing: boolean;
  warnings: string[];
}

export interface ManifestProgress {
  gameId: number;
  filesDone: number;
  filesTotal: number;
  bytesDone: number;
  bytesTotal: number;
  message: string;
}

export interface ManifestReport {
  gameId: number;
  files: number;
  bytes: number;
  builtAt: string;
}

/** How a GameVault folder matched the library (#129). */
export type GameVaultMatchStatus = "exact" | "containment" | "ambiguous" | "none";

export interface GameVaultCandidate {
  folder: string;
  name: string;
  /** Configured GameVault root this folder was found under (#134). */
  sourceRoot: string;
  /** Executable Ludex would launch now; null means "link anyway and choose". */
  executable: string | null;
  /** Every plausible executable found, best first (#134). */
  executableCandidates: string[];
  sizeBytes: number;
  fileCount: number;
  /** Title read from an optional `gamevault.json`, never written. */
  metadataTitle: string | null;
  matchedGameId: number | null;
  matchedTitle: string | null;
  matchStatus: GameVaultMatchStatus;
  /** `adopted`, `ludex` or a launcher id when the folder is already indexed. */
  installed: string | null;
  /** Informational diagnostics: skipped subfolders, depth, chosen candidate. */
  notes: string[];
  /** Problems that need attention: unreadable folders, broken links. */
  warnings: string[];
}

/** Status of one configured root in a multi-root scan (#134). */
export interface GameVaultRootScan {
  root: string;
  candidates: number;
  warnings: string[];
  /** Set when the root itself could not be read or prepared. */
  error: string | null;
}

/** A GameVault root proposed by the read-only auto-detection (#134). */
export interface GameVaultRootProposal {
  path: string;
  /** Why it is proposed: usual location, GameVault config file… */
  reason: string;
  exists: boolean;
  /** Already in the configured list: nothing to do. */
  configured: boolean;
  /** Immediate subfolders that look like game folders. */
  gameFolders: number;
}

export interface GameVaultScan {
  /** One status per configured root, in configured order. */
  roots: GameVaultRootScan[];
  candidates: GameVaultCandidate[];
  warnings: string[];
  /** False when the server could not be listed, so nothing was matched. */
  matched: boolean;
  /** Folder depth used for this scan (4-8). */
  maxDepth: number;
}

export interface GameVaultAdoptEntry {
  folder: string;
  gameId: number;
  versionId?: number | null;
  version?: string | null;
  /** Executable chosen among the ranked candidates; empty = detected. */
  executable?: string | null;
}

export interface GameVaultAdoptSkip {
  folder: string;
  reason: string;
}

export interface GameVaultAdoptReport {
  adopted: InstalledGame[];
  skipped: GameVaultAdoptSkip[];
}

/* --------------------------- desktop extras ------------------------------ */

export interface InstallerPlan {
  executable: string | null;
  /** Installer-looking executables found in the install folder. */
  detected: string[];
  attempts: string[][];
  timeoutSecs: number;
  interactive: boolean;
  platform: string;
  /** Linux: compatibility tool a run would go through (#93). */
  compatTool?: string | null;
  /** Wine prefix the installer would write into. */
  prefix?: string | null;
}

export interface InstallerAttempt {
  args: string[];
  exitCode: number | null;
  timedOut: boolean;
  cancelled: boolean;
  error: string | null;
  durationMs: number;
}

export interface InstallerReport {
  gameId: number;
  executable: string;
  success: boolean;
  cancelled: boolean;
  timedOut: boolean;
  interactive: boolean;
  exitCode: number | null;
  attempts: InstallerAttempt[];
  recordedExecutable: string | null;
  candidates: string[];
  /** Linux compatibility tool used for the run (#93). */
  compatTool?: string | null;
  /** Last lines of the installer log captured on Linux. */
  logTail?: string | null;
  message: string;
}

export interface PresenceStatus {
  enabled: boolean;
  configured: boolean;
  connected: boolean;
  lastError: string | null;
  details: string | null;
  retryInSeconds: number;
}

export interface SteamGame {
  appId: number;
  name: string;
  installDir: string;
  libraryPath: string;
  sizeBytes: number;
  lastPlayed: number | null;
  /** Server game matched by title, when unambiguous. */
  matchedGameId: number | null;
  matchedTitle: string | null;
  imported: boolean;
  /** Local id used when nothing matched (negative app id). */
  localGameId: number;
}

export interface SteamPreview {
  roots: string[];
  games: SteamGame[];
  warnings: string[];
  /** False when the server could not be listed, so nothing is pre-matched. */
  matched: boolean;
}

export interface SteamSkip {
  appId: number;
  title: string;
  reason: string;
}

export interface SteamImportReport {
  imported: InstalledGame[];
  skipped: SteamSkip[];
}

/** Launchers the desktop importer can read (#116). */
export type LauncherId =
  | "steam"
  | "epic"
  | "gog"
  | "ea"
  | "ubisoft"
  | "xbox"
  | "itch"
  | "heroic"
  | "lutris";

/** One game another launcher installed on this machine (#116). */
export interface LauncherGame {
  source: LauncherId;
  externalId: string;
  name: string;
  installDir: string | null;
  /** URI that starts the game through its launcher. */
  launchUri: string | null;
  sizeBytes: number;
  lastPlayed: number | null;
  /** Launcher-side detail (Epic namespace, GOG product id, cave id). */
  detail: string | null;
  /** Why the entry cannot be imported without a manual launch URI. */
  issue: string | null;
  /** Server game matched by title, when unambiguous. */
  matchedGameId: number | null;
  matchedTitle: string | null;
  imported: boolean;
  /** Id used when nothing matched. */
  localGameId: number;
}

export interface LauncherSource {
  id: LauncherId;
  label: string;
  found: number;
  available: boolean;
  /** True when the user turned this source off (#155): nothing was read. */
  disabled: boolean;
  detail: string;
}

export interface LauncherPreview {
  roots: string[];
  games: LauncherGame[];
  sources: LauncherSource[];
  warnings: string[];
  /** False when the server could not be listed, so nothing is pre-matched. */
  matched: boolean;
}

/** One selected launcher game sent to `launcher_import`. */
export interface LauncherImportEntry {
  source: LauncherId;
  externalId: string;
  /** Manual mapping to a Ludex game, overriding title matching. */
  gameId?: number | null;
  /** Manual launch URI for launchers without a documented deep link. */
  launchUri?: string | null;
}

export interface LauncherSkip {
  source: string;
  externalId: string;
  title: string;
  reason: string;
}

export interface LauncherImportReport {
  imported: InstalledGame[];
  skipped: LauncherSkip[];
}

/**
 * "interrupted" is a desktop-only state for jobs persisted across an app
 * restart; the server contract only defines the other seven.
 */
export type InstallPhase =
  | "queued"
  | "downloading"
  | "extracting"
  | "finalizing"
  | "paused"
  | "done"
  | "cancelled"
  | "error"
  | "interrupted";

export interface InstallJob {
  jobId: string;
  gameId: number;
  versionId: number;
  phase: InstallPhase;
  bytesDone: number;
  bytesTotal: number | null;
  speedBps: number;
  message: string | null;
  installPath: string | null;
  /** True when the job used a stream install instead of download+extract (#114). */
  stream: boolean;
  /** True when the stream install has per-entry checkpoints and can resume (#115). */
  resumable: boolean;
  /** Completed / total file entries of a per-entry stream install. */
  entriesDone: number | null;
  entriesTotal: number | null;
  /** Entry currently being written, relative to the install folder. */
  currentEntry: string | null;
  /** Files already present when this attempt started (#115). */
  resumedEntries: number | null;
}

/** A game with a newer version on the server (#30). */
export interface GameUpdate {
  gameId: number;
  title: string;
  installedVersionId: number;
  installedVersion: string | null;
  latestVersionId: number;
  latestVersionLabel: string | null;
  archiveSize: number;
  archiveHash: string | null;
  updatedAt: string | null;
  reason: "newVersion" | "replacedArchive";
}

export interface PlaySession {
  id: string;
  gameId: number;
  startedAt: string;
  endedAt: string | null;
  minutes: number;
  seconds: number;
}

export interface DayPlaytime {
  day: string;
  seconds: number;
}

export interface GamePlaytime {
  gameId: number;
  totalSeconds: number;
  sessions: number;
  firstPlayedAt: string | null;
  lastPlayedAt: string | null;
  byDay: DayPlaytime[];
  recentSessions: PlaySession[];
}

export interface UpdateInfo {
  version: string;
  currentVersion: string;
  notes: string | null;
  publishedAt: string | null;
  /** Release page for this build (#163). */
  releaseUrl: string;
  /** True for `edge` builds such as `0.3.2-edge.179`. */
  isPrerelease: boolean;
  /** CI run number of an `edge` build, when the version carries one. */
  build: number | null;
}

/** Result of an application update check (#16, #163). */
export interface UpdateStatus {
  configured: boolean;
  channel: "stable" | "early";
  currentVersion: string;
  lastCheckedAt: string | null;
  /** GitHub endpoint the check queried, shown in the update center. */
  endpoint: string;
  /** Version the user asked not to be prompted about (#163). */
  skippedVersion: string | null;
  /** False for a skipped version: no toast, no badge, no startup prompt. */
  notify: boolean;
  available: UpdateInfo | null;
  message: string | null;
}

export interface UpdateProgress {
  downloaded: number;
  total: number | null;
  percent: number | null;
  /** Instantaneous download speed in bytes/s. */
  speedBps: number;
}

export interface UpdateInstalled {
  version: string;
  restarted: boolean;
}

export interface DiskUsage {
  totalBytes: number;
  freeBytes: number;
}

export interface InstallGameArgs {
  gameId: number;
  versionId: number;
  serverUrl: string;
  token: string;
  title: string;
  version?: string | null;
  executable?: string | null;
  installDir?: string | null;
  downloadDir?: string | null;
  /** Named location for the install root (#99). */
  locationId?: string | null;
  /** Named location for the temporary archive (#99). */
  downloadLocationId?: string | null;
  /** Extracted size, used by the free-space guard (#99). */
  expectedBytes?: number | null;
  stream?: boolean;
}

export interface VerifyIssue {
  path: string;
  expectedSize: number;
  actualSize: number | null;
  expectedSha256: string;
  actualSha256: string | null;
}

export interface VerifyExtra {
  path: string;
  size: number;
}

export interface VerifyReport {
  gameId: number;
  status: "ok" | "issues" | "noManifest";
  filesChecked: number;
  bytesChecked: number;
  missing: VerifyIssue[];
  corrupt: VerifyIssue[];
  extra: VerifyExtra[];
  checkedAt: string;
  message: string | null;
}

export interface RepairReport {
  gameId: number;
  repaired: string[];
  failed: string[];
  /** Ludex could not repair in place; a full reinstall is required. */
  fallback: boolean;
  message: string;
  verify: VerifyReport;
}

export interface MoveProgress {
  gameId: number;
  bytesDone: number;
  bytesTotal: number;
  message: string;
}

export interface VerifyProgress {
  gameId: number;
  filesDone: number;
  filesTotal: number;
  bytesDone: number;
  bytesTotal: number;
  message: string;
}

export interface RepairProgress extends VerifyProgress {
  phase: "downloading" | "extracting" | "restoring" | "verifying";
}

/* --------------------- Linux compatibility (#89-#93) --------------------- */

export type LaunchMode = "auto" | "native" | "proton" | "umu";

/** How a game is started on Linux; stored locally per game. */
export interface LaunchOptions {
  mode: LaunchMode;
  /** Preferred `compat_scan` tool id; empty = best available. */
  toolId: string;
  /** Custom Wine prefix; empty = `{installPath}/.prefix`. */
  prefixPath: string;
  /** Extra environment variables (`DXVK_*`, `PROTON_*`, `WINEDLLOVERRIDES`…). */
  env: Record<string, string>;
  /** Winetricks verbs run on demand in the game prefix. */
  winetricksVerbs: string[];
  /** Steam app id for ProtonDB and `GAMEID`. */
  steamAppId: number | null;
}

export interface CompatTool {
  id: string;
  name: string;
  kind: "umu" | "proton" | "wine";
  version: string | null;
  /** Executable: `umu-run`, the `proton` script or the `wine` binary. */
  path: string;
  protonPath: string | null;
  /** `path`, `steam`, `compatibilitytools.d` or `override`. */
  source: string;
  steamRoot: string | null;
}

export interface CompatScan {
  platform: string;
  /** True on Linux, where the compatibility panel applies. */
  linux: boolean;
  steamRoots: string[];
  tools: CompatTool[];
  umu: string | null;
  proton: string | null;
  wine: string | null;
  winetricks: string | null;
  protonfixes: string | null;
  defaultMode: LaunchMode;
  steamClientPath: string | null;
  suggestedVerbs: string[];
  warnings: string[];
}

export interface PrefixInfo {
  gameId: number;
  path: string;
  custom: boolean;
  exists: boolean;
  sizeBytes: number;
}

export interface WinetricksReport {
  verbs: string[];
  success: boolean;
  timedOut: boolean;
  exitCode: number | null;
  output: string;
  durationMs: number;
}

export type ProtonDbStatus = "ok" | "cached" | "stale" | "unavailable" | "notFound" | "noAppId";

/** ProtonDB summary for a Steam app id (#91). */
export interface ProtonDbRating {
  appId: number | null;
  status: ProtonDbStatus;
  tier: string | null;
  bestTier: string | null;
  trendingTier: string | null;
  confidence: string | null;
  total: number;
  score: number | null;
  url: string | null;
  fetchedAt: string | null;
  message: string | null;
}

/* ------------------------------ API security ------------------------------ */

export interface AuthSession {
  id: number;
  userAgent: string | null;
  ip: string | null;
  createdAt: string;
  lastSeenAt: string;
  expiresAt: string;
  revokedAt: string | null;
  /** True for the session used by the current request. */
  current: boolean;
}

/* ------------------------------- uploads --------------------------------- */

export interface UploadSession {
  id: string;
  fileName: string;
  size: number;
  sha256: string | null;
  chunkSize: number;
  totalParts: number;
  receivedBytes: number;
  receivedParts: number[];
  status: "open" | "complete" | "failed" | string;
  createdAt: string;
  updatedAt: string;
  gameId: number | null;
  versionId: number | null;
  error: string | null;
}

export interface UploadPartResponse {
  id: string;
  part: number;
  receivedBytes: number;
  receivedParts: number;
  totalParts: number;
}

export interface CompleteUploadResponse {
  id: string;
  gameId: number;
  versionId: number;
  fileName: string;
  size: number;
  sha256: string | null;
}

/* ------------------------------ achievements ----------------------------- */

/** One achievement definition plus the requesting account's state. */
export interface Achievement {
  id: number;
  /** Owning game; null for a global achievement. */
  gameId: number | null;
  gameTitle: string | null;
  coverUrl: string | null;
  key: string;
  /** Masked to "Hidden achievement" while secret and locked. */
  name: string;
  description: string | null;
  iconUrl: string | null;
  points: number;
  hidden: boolean;
  /** `manual`, `game_files`, `gog_galaxy_api` or `gog_galaxy_sqlite`. */
  provider: string;
  unlocked: boolean;
  unlockedAt: string | null;
  /** Completion fraction, 0..1. */
  progress: number;
  /** Share of accounts that unlocked it, 0..100. */
  unlockPercentage: number;
  /** Source of the unlock when unlocked. */
  source: string | null;
}

export interface AchievementScanSummary {
  scannedAt: string;
  providers: string[];
  found: number;
  unlocked: number;
}

export interface GameAchievements {
  gameId: number;
  total: number;
  unlocked: number;
  points: number;
  unlockedPoints: number;
  /** Completion percentage, 0..100. */
  completion: number;
  achievements: Achievement[];
  lastScan: AchievementScanSummary | null;
}

export interface AchievementUser {
  id: number;
  username: string;
  displayName: string | null;
  publicProfile: boolean;
}

export interface UserAchievements {
  user: AchievementUser;
  unlocked: number;
  unlockedPoints: number;
  games: number;
  achievements: Achievement[];
}

export interface AchievementProviderReport {
  provider: string;
  status: "ok" | "skipped" | "error";
  found: number;
  unlocked: number;
  message: string | null;
}

export interface AchievementScanReport {
  gameId: number;
  userId: number;
  scannedAt: string;
  found: number;
  unlocked: number;
  providers: AchievementProviderReport[];
  warnings: string[];
}

/* ------------------- game settings editor (#105, #106) ------------------- */

export interface ConfigFileInfo {
  /** Path relative to the install folder. */
  path: string;
  name: string;
  format: "ini" | "cfg" | "toml" | "json";
  sizeBytes: number;
  entries: number;
  editable: boolean;
  note: string | null;
}

export interface ConfigEntry {
  /** `Section/key`, `section.key` or `a.b[0]`. */
  key: string;
  value: string;
  valueType: "string" | "number" | "boolean";
  section: string | null;
  line: number;
}

export interface ConfigDocument {
  path: string;
  name: string;
  format: "ini" | "cfg" | "toml" | "json";
  sizeBytes: number;
  entries: ConfigEntry[];
  note: string | null;
}

export interface ConfigEditInput {
  key: string;
  value: string;
}

/** One backup in the per-game history; `backupPath` stays on this machine. */
export interface ConfigBackupRecord {
  id: string;
  gameId: number;
  filePath: string;
  backupPath: string;
  diff: string;
  bytes: number;
  origin: "manual" | "adapter" | "restore" | "common-field" | "sync";
  createdAt: string;
}

export interface ConfigWriteReport {
  document: ConfigDocument;
  diff: string;
  backup: ConfigBackupRecord;
}

export interface GameConfigListing {
  gameId: number;
  installPath: string;
  files: ConfigFileInfo[];
}

export interface AdapterOption {
  value: string;
  label: string;
}

export interface AdapterField {
  id: string;
  label: string;
  control: "select" | "toggle" | "number" | "text";
  value: string | null;
  options: AdapterOption[];
  file: string | null;
  key: string | null;
  valueType: "string" | "number" | "boolean";
  note: string | null;
}

export interface AdapterFile {
  path: string;
  label: string;
  exists: boolean;
}

export interface ConfigAdapter {
  id: "unreal" | "gog" | "godot" | "unity";
  label: string;
  detected: boolean;
  evidence: string | null;
  files: AdapterFile[];
  fields: AdapterField[];
  notes: string[];
}

export interface AdapterReport {
  gameId: number;
  installPath: string;
  adapters: ConfigAdapter[];
  notes: string[];
}

/* ------------------ common config fields (#148) -------------------- */

/** One key that may hold a common field. */
export interface CommonFieldCandidate {
  /** Path relative to the install folder. */
  file: string;
  key: string;
  value: string;
  valueType: "string" | "number" | "boolean";
  line: number;
  section: string | null;
  /** True when the key itself, not its section, matched the catalog. */
  exact: boolean;
}

/**
 * A detected language/player-name field. `file`, `key` and `value` are null
 * when `confidence` is `ambiguous`: Ludex lists the candidates and never picks.
 */
export interface CommonFieldMatch {
  field: "language" | "playerName";
  label: string;
  confidence: "certain" | "likely" | "ambiguous";
  source: "builtin" | "template";
  file: string | null;
  key: string | null;
  value: string | null;
  valueType: "string" | "number" | "boolean" | null;
  /** Values the file itself lists as accepted (GOG `langs[n]`), else empty. */
  allowedValues: string[];
  candidates: CommonFieldCandidate[];
  note: string | null;
}

/** Pinned file/key overriding the catalog for one game. */
export interface ConfigFieldMapping {
  gameId: number;
  field: "language" | "playerName";
  /** Relative to the install folder; serialized as `file`. */
  file: string;
  key: string;
  createdAt: string;
}

export interface CommonFieldsReport {
  gameId: number;
  installPath: string;
  /** Empty means no panel at all. */
  fields: CommonFieldMatch[];
  mappings: ConfigFieldMapping[];
  notes: string[];
}

/** What to write: the field id plus the desired language code or player name. */
export interface CommonFieldTarget {
  field: "language" | "playerName";
  value: string;
}

export interface CommonFieldPreview {
  field: "language" | "playerName";
  file: string;
  key: string;
  currentValue: string;
  nextValue: string;
  unchanged: boolean;
  diff: string;
}

/** Per-game launch-time sync state (#148). Off until the user opts in. */
export interface ConfigSyncPrefs {
  gameId: number;
  enabled: boolean;
  language: string;
  playerName: string;
  applyLanguage: boolean;
  applyPlayerName: boolean;
  lastAppliedAt: string | null;
  updatedAt: string;
}

export interface ConfigSyncInput {
  enabled: boolean;
  language?: string;
  playerName?: string;
  applyLanguage?: boolean;
  applyPlayerName?: boolean;
}

/* ---------------------------------- Controller profiles (#51) ---------------------------------- */

export type ControllerAction =
  | "left"
  | "right"
  | "up"
  | "down"
  | "confirm"
  | "back"
  | "menu"
  | "downloads"
  | "prevTab"
  | "nextTab";

/** Per-input to logical-action mapping. Keys are `buttonN`, `axisN-`, `axisN+`. */
export interface ControllerProfile {
  name: string;
  deadzone: number;
  sensitivity: number;
  curve: number;
  invertX: boolean;
  invertY: boolean;
  triggerThreshold: number;
  vibration: boolean;
  mappings: Record<string, ControllerAction | string>;
}

export interface ControllerProfileEntry {
  gameId: number | null;
  gameTitle: string | null;
  profile: ControllerProfile;
  updatedAt: string;
}

export interface ControllerPreset {
  id: string;
  label: string;
  profile: ControllerProfile;
}

export interface ControllerProfilesResponse {
  global: ControllerProfileEntry | null;
  games: ControllerProfileEntry[];
  presets: ControllerPreset[];
}

/* ---------------------------------- Emulators and ROMs (#52) ---------------------------------- */

export interface PlatformInfo {
  id: string;
  label: string;
  extensions: string[];
  retroarchCore: string | null;
  gameCount: number;
}

export interface Emulator {
  id: number;
  name: string;
  command: string;
  platforms: string[];
  enabled: boolean;
  createdAt: string;
  updatedAt: string;
}

export interface EmulatorDefault {
  platform: string;
  emulatorId: number | null;
}

export interface EmulatorLaunch {
  gameId: number;
  platform: string;
  emulatorId: number;
  emulatorName: string;
  program: string;
  args: string[];
  romPath: string;
}

/** Desktop-local emulator registry (#52). */
export interface RomEmulator {
  id: string;
  name: string;
  command: string;
  platforms: string[];
  enabled: boolean;
  builtin: boolean;
}

export interface RomSettings {
  registry: {
    emulators: RomEmulator[];
    defaults: Record<string, string>;
  };
  platforms: { id: string; label: string }[];
}

export interface RomLaunchResult {
  sessionId: string;
  emulatorName: string;
  emulatorId: string;
}

export interface RomCommandValidation {
  valid: boolean;
  error: string | null;
  program: string | null;
  args: string[];
}

/* ---------------------------------- Backups (#169) ---------------------------------- */

export type BackupKind = "manual" | "pre_restore" | "other";
export type BackupVerificationState = "unverified" | "verified" | "failed";

export interface BackupVerification {
  state: BackupVerificationState;
  checkedAt: string | null;
  message: string | null;
  games: number | null;
  versions: number | null;
  users: number | null;
  saves: number | null;
  files: number | null;
  bytes: number | null;
  serverVersion: string | null;
}

export interface BackupEntry {
  name: string;
  path: string;
  size: number;
  createdAt: string;
  kind: BackupKind;
  verification: BackupVerification;
}

export interface BackupList {
  directory: string;
  backups: BackupEntry[];
}

export interface BackupCreateInput {
  dryRun?: boolean;
  includeMedia?: boolean;
  uploadS3?: boolean;
}

export interface BackupCreated {
  name: string;
  path: string;
  bytes: number;
  files: number;
  dryRun: boolean;
  uploadedKey: string | null;
  createdAt: string;
}

export interface BackupRestoreInput {
  dryRun?: boolean;
  includeMedia?: boolean;
  confirmation?: string;
}

export interface BackupRestoreResult {
  dryRun: boolean;
  overwrote: boolean;
  files: number;
  bytes: number;
  games: number;
  versions: number;
  users: number;
  createdAt: string;
  safetyBackup: string | null;
  restartRequired: boolean;
}

/* --------------------------------- Observability (#170) --------------------------------- */

/** Time window supported by the admin dashboard. */
export type ObservabilityWindow = "1h" | "24h";

export interface ObservabilityServer {
  version: string;
  commit: string;
  uptimeSeconds: number;
  startedAt: string;
}

export interface ObservabilityRequests {
  total: number;
  perSecond: number;
  success: number;
  redirects: number;
  clientErrors: number;
  serverErrors: number;
  /** Share of 4xx/5xx responses, 0..1. */
  errorRate: number;
  averageMs: number;
  p50Ms: number;
  p95Ms: number;
  p99Ms: number;
}

export interface ObservabilityPoint {
  at: string;
  requests: number;
  success: number;
  redirects: number;
  clientErrors: number;
  serverErrors: number;
  averageMs: number;
}

export interface ObservabilityErrorRoute {
  method: string;
  route: string;
  status: number;
  count: number;
}

export interface ObservabilityActive {
  scans: number;
  ingestRunning: number;
  ingestQueued: number;
  downloads: number;
  integrityRunning: boolean;
}

export interface ObservabilityDatabase {
  ok: boolean;
  backend: string;
  sizeBytes: number;
  latencyMs: number;
  connections: number;
  idleConnections: number;
  maxConnections: number;
}

export interface ObservabilityDisk {
  dataFreeBytes: number;
  gamesFreeBytes: number;
  mediaFreeBytes: number;
}

export interface ObservabilityMedia {
  files: number;
  bytes: number;
}

export interface ObservabilitySaves {
  snapshots: number;
  bytes: number;
}

export interface ObservabilityBackups {
  archives: number;
  bytes: number;
  latestAt: string | null;
}

export interface ObservabilityRoot {
  label: string;
  enabled: boolean;
  available: boolean;
  versions: number;
  lastError: string | null;
}

export interface ObservabilityStorage {
  backend: string;
  disk: ObservabilityDisk;
  media: ObservabilityMedia;
  saves: ObservabilitySaves;
  backups: ObservabilityBackups;
  roots: ObservabilityRoot[];
}

export interface ObservabilityEvent {
  id: number;
  action: string;
  actorKind: "user" | "api_key" | "system" | string;
  targetType: string | null;
  targetId: string | null;
  createdAt: string | null;
}

export interface ObservabilityWebhookFailure {
  id: number;
  event: string;
  statusCode: number | null;
  attempts: number;
  lastError: string | null;
  nextAttemptAt: string | null;
}

export interface ObservabilityWebhooks {
  pending: number;
  failed: number;
  recentFailures: ObservabilityWebhookFailure[];
}

export interface ObservabilityFinding {
  kind: string;
  targetType: string;
  targetId: string | null;
  detail: string | null;
  createdAt: string | null;
}

export interface ObservabilityIntegrity {
  last: {
    id: number;
    status: string;
    findings: number;
    startedAt: string | null;
    finishedAt: string | null;
  } | null;
  recentFindings: ObservabilityFinding[];
}

export interface ObservabilityPrometheus {
  endpoint: string;
  tokenProtected: boolean;
}

export interface ObservabilitySnapshot {
  generatedAt: string;
  window: ObservabilityWindow;
  server: ObservabilityServer;
  requests: ObservabilityRequests;
  series: ObservabilityPoint[];
  topErrors: ObservabilityErrorRoute[];
  active: ObservabilityActive;
  database: ObservabilityDatabase;
  storage: ObservabilityStorage;
  recentEvents: ObservabilityEvent[];
  webhooks: ObservabilityWebhooks;
  integrity: ObservabilityIntegrity;
  prometheus: ObservabilityPrometheus;
}
