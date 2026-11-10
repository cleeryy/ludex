/**
 * Types mirror docs/CONTRACT.md exactly. Keep this file in sync with the
 * contract; everything else in the app imports from here.
 */

export type Role = "admin" | "editor" | "user";

export type ProgressState = "unplayed" | "playing" | "completed" | "abandoned" | "onHold" | "planToPlay";

export interface User {
  id: number;
  username: string;
  email: string | null;
  role: Role;
  activated: boolean;
  avatarUrl: string | null;
  createdAt: string;
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
  label: string;
  fileName: string;
  size: number;
  sha256: string;
  createdAt: string;
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
  | "collection_created";

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

export interface LocalSettings {
  mode: "local" | "remote";
  serverUrl: string;
  token: string | null;
  gamesDir: string | null;
  installDir: string;
  downloadDir: string;
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
  source?: "ludex" | "steam";
  /** External launch URI for imported entries (`steam://rungameid/…`). */
  launchUri?: string | null;
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
  /** True when the job used the non-resumable stream install (#114). */
  stream: boolean;
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
