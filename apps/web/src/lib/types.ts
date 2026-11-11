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

export interface LocalSettings {
  mode: "local" | "remote";
  serverUrl: string;
  token: string | null;
  gamesDir: string | null;
  installDir: string;
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
  configFound: boolean;
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
  executable: string | null;
  sizeBytes: number;
  fileCount: number;
  /** Title read from an optional `gamevault.json`, never written. */
  metadataTitle: string | null;
  matchedGameId: number | null;
  matchedTitle: string | null;
  matchStatus: GameVaultMatchStatus;
  /** `adopted`, `ludex` or a launcher id when the folder is already indexed. */
  installed: string | null;
  notes: string[];
}

export interface GameVaultScan {
  root: string;
  candidates: GameVaultCandidate[];
  warnings: string[];
  /** False when the server could not be listed, so nothing was matched. */
  matched: boolean;
}

export interface GameVaultAdoptEntry {
  folder: string;
  gameId: number;
  versionId?: number | null;
  version?: string | null;
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
}

/** Result of an application update check (#16). */
export interface UpdateStatus {
  configured: boolean;
  channel: "stable" | "early";
  currentVersion: string;
  lastCheckedAt: string | null;
  available: UpdateInfo | null;
  message: string | null;
}

export interface UpdateProgress {
  downloaded: number;
  total: number | null;
  percent: number | null;
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
  origin: "manual" | "adapter" | "restore";
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
