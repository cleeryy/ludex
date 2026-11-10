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

export interface GameDetail extends GameSummary {
  description: string | null;
  screenshots: string[];
  trailers: { title: string; url: string; thumbnailUrl: string | null }[];
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
