// This file is generated from the server OpenAPI document.
// Regenerate with `node scripts/gen-types.mjs --write`; CI fails when it is stale.
// Do not edit by hand.

export interface AcceptInvitationRequest {
  password: string;
  username: string;
}

export interface Accepted {
  started: boolean;
}

export interface AccountProfileRequest {
  locale?: string | null;
}

export interface Achievement {
  coverUrl?: string | null;
  description?: string | null;
  gameId?: number | null;
  gameTitle?: string | null;
  hidden: boolean;
  iconUrl?: string | null;
  id: number;
  key: string;
  name: string;
  points: number;
  progress: number;
  provider: string;
  source?: string | null;
  unlockPercentage: number;
  unlocked: boolean;
  unlockedAt?: string | null;
}

export interface AchievementUser {
  displayName?: string | null;
  id: number;
  publicProfile: boolean;
  username: string;
}

export interface Activity {
  collection?: ActivityCollection | null;
  createdAt: string;
  game?: ActivityGame | null;
  id: number;
  kind: string;
  metadata: Record<string, unknown>;
  user: ActivityUser;
}

export interface ActivityCollection {
  id: number;
  name: string;
}

export interface ActivityGame {
  coverUrl?: string | null;
  id: number;
  releaseYear?: number | null;
  title: string;
}

export interface ActivityPage {
  items: Activity[];
  nextBefore?: number | null;
}

export interface ActivityUser {
  avatarUrl?: string | null;
  displayName?: string | null;
  id: number;
  username: string;
}

export interface AdminGameCounts {
  available: number;
  missing: number;
  partiallyMissing: number;
  total: number;
}

export interface AdminGameRow {
  availableVersionCount: number;
  coverUrl?: string | null;
  createdAt: string;
  defaultVersionId?: number | null;
  id: number;
  installedReports: number;
  kind: string;
  lastInstalledAt?: string | null;
  missingVersionCount: number;
  platform?: string | null;
  providerSlug?: string | null;
  releaseDate?: string | null;
  status: string;
  title: string;
  totalSize: number;
  updatedAt: string;
}

export interface AdminGamesResponse {
  counts: AdminGameCounts;
  items: AdminGameRow[];
  limit: number;
  page: number;
  total: number;
}

export interface ApiKey {
  createdAt: string;
  id: number;
  lastUsedAt?: string | null;
  name: string;
  prefix: string;
  revokedAt?: string | null;
  scopes: string[];
  userId: number;
}

export interface ApplyMetadataRequest {
  provider: string;
  providerId: string;
  replaceManual?: boolean;
}

export interface ArtworkResponse {
  format: string;
  kind: string;
  sha256: string;
  size: number;
  url: string;
}

export interface AuditLog {
  action: string;
  actorId?: number | null;
  actorKind: string;
  createdAt: string;
  id: number;
  ip?: string | null;
  metadata: Record<string, unknown>;
  targetId?: string | null;
  targetType?: string | null;
}

export interface AuditPage {
  items: AuditLog[];
  nextBefore?: number | null;
}

export interface AuthResponse {
  challengeExpiresAt?: string | null;
  challengeToken?: string | null;
  expiresAt?: string | null;
  mfaRequired?: boolean;
  token?: string | null;
  user?: User | null;
}

export interface AuthSession {
  createdAt: string;
  current: boolean;
  device?: string | null;
  expiresAt: string;
  id: number;
  ip?: string | null;
  lastSeenAt: string;
  revokedAt?: string | null;
  userAgent?: string | null;
}

export interface BackupCreated {
  bytes: number;
  createdAt: string;
  dryRun: boolean;
  files: number;
  name: string;
  path: string;
  uploadedKey?: string | null;
}

export interface BackupEntry {
  createdAt: string;
  kind: string;
  name: string;
  path: string;
  size: number;
  verification: BackupVerification;
}

export interface BackupList {
  backups: BackupEntry[];
  directory: string;
}

export interface BackupVerification {
  bytes?: number | null;
  checkedAt?: string | null;
  files?: number | null;
  games?: number | null;
  message?: string | null;
  saves?: number | null;
  serverVersion?: string | null;
  state: string;
  users?: number | null;
  versions?: number | null;
}

export interface BookmarkRequest {
  bookmarked: boolean;
}

export interface BookmarkResponse {
  bookmarked: boolean;
}

export interface BulkItemResult {
  code?: string | null;
  deletedFiles: number;
  deletedVersions: number;
  freedBytes: number;
  gameId: number;
  message?: string | null;
  ok: boolean;
}

export interface BulkJobView {
  action: string;
  done: number;
  failed: number;
  finishedAt?: string | null;
  id: string;
  results: BulkItemResult[];
  startedAt: string;
  status: string;
  succeeded: number;
  total: number;
}

export interface BulkPreview {
  bytes: number;
  files: number;
  games: number;
  items: BulkPreviewItem[];
}

export interface BulkPreviewItem {
  availableVersionCount: number;
  bytes: number;
  files: number;
  gameId: number;
  missingVersionCount: number;
  paths: string[];
  title: string;
  versions: number;
}

export interface BulkRequest {
  action: string;
  confirm?: boolean;
  dryRun?: boolean;
  ids?: number[];
}

export interface BulkResponse {
  action: string;
  dryRun: boolean;
  jobId?: string | null;
  preview?: BulkPreview | null;
  results: BulkItemResult[];
  totals: BulkTotals;
}

export interface BulkTotals {
  bytes: number;
  failed: number;
  files: number;
  games: number;
  succeeded: number;
}

export interface CacheEntryView {
  bytes: number;
  createdAt: string;
  files: number;
  lastUsedAt: string;
  sha256: string;
  versionId: number;
}

export interface CacheOverview {
  budgetBytes: number;
  enabled: boolean;
  entries: CacheEntryView[];
  root: string;
  usedBytes: number;
}

export interface ChangePasswordRequest {
  currentPassword: string;
  newPassword: string;
}

export interface Collection {
  createdAt: string;
  description?: string | null;
  gameCount: number;
  id: number;
  isPublic: boolean;
  kind: CollectionKind;
  name: string;
  owned: boolean;
  owner: CollectionOwner;
  rules: SmartRules;
  updatedAt: string;
  visibility: CollectionVisibility;
}

export interface CollectionGamesRequest {
  gameIds: number[];
}

export interface CollectionGamesResponse {
  added: number;
  total: number;
}

export type CollectionKind = "manual" | "smart";

export interface CollectionOwner {
  displayName?: string | null;
  id: number;
  username: string;
}

export interface CollectionShareAccessRequest {
  password?: string | null;
}

export interface CollectionShareLink {
  collectionId: number;
  collectionName: string;
  createdAt: string;
  expiresAt?: string | null;
  hasPassword: boolean;
  id: number;
  label?: string | null;
  revokedAt?: string | null;
  token?: string | null;
  viewCount: number;
}

export interface CollectionShareView {
  collection: SharedCollection;
  expiresAt?: string | null;
  games: SharedCollectionGame[];
  hasPassword: boolean;
}

export type CollectionVisibility = "private" | "unlisted" | "public";

export interface CompleteUploadResponse {
  fileName: string;
  gameId: number;
  id: string;
  sha256?: string | null;
  size: number;
  versionId: number;
}

export interface ContentHashStatus {
  failed: number;
  finishedAt?: string | null;
  hashed: number;
  lastError?: string | null;
  pending: number;
  rowsPerSecond?: number | null;
  running: boolean;
  startedAt?: string | null;
  total: number;
}

export interface ControllerProfile {
  curve?: number;
  deadzone?: number;
  invertX?: boolean;
  invertY?: boolean;
  mappings?: Record<string, string>;
  name?: string;
  sensitivity?: number;
  triggerThreshold?: number;
  vibration?: boolean;
}

export interface ControllerProfileEntry {
  gameId?: number | null;
  gameTitle?: string | null;
  profile: ControllerProfile;
  updatedAt: string;
}

export interface ControllerProfilesResponse {
  games: ControllerProfileEntry[];
  global?: ControllerProfileEntry | null;
  presets: PresetEntry[];
}

export interface CreateApiKeyRequest {
  name: string;
  scopes?: string[] | null;
  userId?: number | null;
}

export type CreateApiKeyResponse = ApiKey & {
  secret: string;
};

export interface CreateBackupRequest {
  dryRun?: boolean;
  includeMedia?: boolean;
  uploadS3?: boolean;
}

export interface CreateCollectionRequest {
  description?: string | null;
  gameIds?: number[] | null;
  isPublic?: boolean | null;
  kind?: CollectionKind | null;
  name: string;
  rules?: SmartRules | null;
  visibility?: CollectionVisibility | null;
}

export interface CreateCollectionShareRequest {
  expiresInHours?: number | null;
  label?: string | null;
  password?: string | null;
}

export interface CreateDownloadRequest {
  title?: string | null;
  url: string;
}

export interface CreateEmulatorRequest {
  command: string;
  enabled?: boolean | null;
  name: string;
  platforms?: string[];
}

export interface CreateInvitationRequest {
  email?: string | null;
  expiresInDays?: number | null;
  maxUses?: number | null;
  note?: string | null;
  role?: Role | null;
}

export interface CreateLibraryRootRequest {
  enabled?: boolean | null;
  kind?: string | null;
  label?: string | null;
  path: string;
}

export interface CreateMetadataProviderRequest {
  baseUrl: string;
  enabled?: boolean | null;
  mapping?: Record<string, unknown>;
  name: string;
  priority?: number | null;
  token?: string | null;
}

export interface CreateNewsRequest {
  body: string;
  title: string;
}

export interface CreateRemoteInstallRequest {
  device?: string | null;
  gameId: number;
  targetDeviceId?: string | null;
  versionId?: number | null;
}

export interface CreateShareRequest {
  expiresInHours?: number | null;
  label?: string | null;
  maxDownloads?: number | null;
  password?: string | null;
  versionId?: number | null;
}

export interface CreateUploadRequest {
  fileName: string;
  sha256?: string | null;
  size: number;
}

export interface CreateUserRequest {
  email?: string | null;
  password: string;
  role?: Role | null;
  username: string;
}

export interface CreateWebhookRequest {
  enabled?: boolean | null;
  events: string[];
  url: string;
}

export type CreateWebhookResponse = Webhook & {
  secret: string;
};

export interface CreatedInvitation {
  invitation: Invitation;
  path: string;
  token: string;
}

export interface DefinitionInput {
  description?: string | null;
  hidden?: boolean | null;
  iconPath?: string | null;
  key: string;
  name: string;
  points?: number | null;
}

export interface DefinitionPatch {
  description?: string | null;
  hidden?: boolean | null;
  iconPath?: string | null;
  name?: string | null;
  points?: number | null;
}

export interface DefinitionResponse {
  gameId: number;
  key: string;
}

export interface DeleteGameResponse {
  deletedFiles: number;
  freedBytes: number;
  gameId: number;
  keptFiles: number;
  missingVersions: number;
  mode: string;
  versions: number;
}

export interface DeleteVersionResponse {
  defaultVersionId?: number | null;
  freedBytes: number;
  versionId: number;
}

export interface DesktopAsset {
  kind: string;
  name: string;
  platform: string;
  sha256?: string | null;
  sizeBytes: number;
  url: string;
}

export interface DesktopDownload {
  assets: DesktopAsset[];
  channel: string;
  checksumsUrl?: string | null;
  notes?: string | null;
  platform?: string | null;
  recommended?: DesktopAsset | null;
  releaseUrl: string;
  releasedAt?: string | null;
  source: string;
  stale: boolean;
  tag?: string | null;
  version?: string | null;
}

export interface DesktopHandoffClaimRequest {
  state: string;
}

export interface DesktopHandoffClaimResponse {
  code: string;
  expiresAt: string;
}

export interface DesktopHandoffRequest {
  state: string;
}

export interface DesktopHandoffResponse {
  expiresAt: string;
}

export interface Device {
  appVersion?: string | null;
  capabilities: DeviceCapabilities;
  createdAt: string;
  freeDiskBytes?: number | null;
  id: string;
  installedCount: number;
  lastSeenAt: string;
  name: string;
  online: boolean;
  platform?: string | null;
  revokedAt?: string | null;
  streamingAvailable: boolean;
}

export interface DeviceCapabilities {
  freeDiskBytes?: number | null;
  installedCount?: number | null;
  installedGameIds?: number[];
  streamingAvailable?: boolean;
}

export interface DeviceHeartbeatRequest {
  appVersion?: string | null;
  capabilities?: DeviceCapabilities;
  clientId: string;
  name?: string | null;
  platform?: string | null;
}

export interface DiagnosticsCount {
  count: number;
  value: string;
}

export interface DiagnosticsPage {
  byKind: DiagnosticsCount[];
  byPlatform: DiagnosticsCount[];
  byVersion: DiagnosticsCount[];
  items: DiagnosticsReport[];
  nextBefore?: number | null;
  retentionDays: number;
  total: number;
}

export interface DiagnosticsReport {
  appVersion: string;
  channel: string;
  createdAt: string;
  detail: Record<string, unknown>;
  errorKind: string;
  id: number;
  installId: string;
  locale: string;
  os: string;
  platform: string;
  summary: string;
}

export interface DiagnosticsReportInput {
  appVersion: string;
  channel?: string;
  detail?: Record<string, unknown>;
  errorKind: string;
  installId: string;
  locale?: string;
  os: string;
  platform: string;
  summary: string;
}

export interface Emulator {
  command: string;
  createdAt: string;
  enabled: boolean;
  id: number;
  name: string;
  platforms: string[];
  updatedAt: string;
}

export interface EmulatorLaunch {
  args: string[];
  emulatorId: number;
  emulatorName: string;
  gameId: number;
  platform: string;
  program: string;
  romPath: string;
}

export interface EngineHit {
  category: string;
  engine: string;
  result?: string | null;
}

export interface ErrorBody {
  code: string;
  message: string;
}

export interface ErrorEnvelope {
  error: ErrorBody;
}

export type ExtractState = "queued" | "extracting" | "ready" | "failed" | "unavailable";

export interface ExtractStatus {
  archiveSha256: string;
  bytes: number;
  error?: string | null;
  files: number;
  status: ExtractState;
  versionId: number;
}

export interface ExtractedFile {
  executable: boolean;
  mode: number;
  path: string;
  sha256: string;
  size: number;
}

export interface FeatureFlags {
  archiveSha256: boolean;
  igdb: boolean;
  ingest: boolean;
  localMode: boolean;
  oidc: boolean;
  steam: boolean;
}

export interface Friend {
  presence?: FriendPresence | null;
  since: string;
  user: SocialUser;
}

export interface FriendGame {
  id: number;
  title: string;
}

export interface FriendPresence {
  game?: FriendGame | null;
  lastSeen?: string | null;
  state: string;
}

export interface FriendPresenceEntry {
  game?: FriendGame | null;
  lastSeen?: string | null;
  state: string;
  userId: number;
}

export interface FriendRequest {
  createdAt: string;
  id: number;
  user: SocialUser;
}

export interface FriendRequestInput {
  userId?: number | null;
  username?: string | null;
}

export interface FriendsPresenceResponse {
  friends: FriendPresenceEntry[];
  pollAfterSeconds: number;
}

export interface FriendsResponse {
  blocked: SocialUser[];
  friends: Friend[];
  incoming: FriendRequest[];
  outgoing: FriendRequest[];
}

export interface GameAchievements {
  achievements: Achievement[];
  completion: number;
  gameId: number;
  lastScan?: ScanSummary | null;
  points: number;
  total: number;
  unlocked: number;
  unlockedPoints: number;
}

export type GameDetail = GameSummary & {
  ageRating?: number | null;
  altTitles?: string[];
  defaultVersionId?: number | null;
  description?: string | null;
  developers: string[];
  launch: LaunchInfo;
  missingVersions?: GameVersion[];
  provider?: ProviderRef | null;
  publishers: string[];
  screenshots: string[];
  tags: string[];
  trailers: Trailer[];
  versions: GameVersion[];
  videos?: Trailer[];
};

export interface GameList {
  items: GameSummary[];
  limit: number;
  page: number;
  total: number;
}

export interface GameScanReport {
  gameId: number;
  versions: ScanReport[];
}

export interface GameSuggestion {
  coverUrl?: string | null;
  id: number;
  matchedField: string;
  matchedTitle: string;
  score: number;
  title: string;
  year?: number | null;
}

export interface GameSummary {
  availableVersionCount?: number;
  backgroundUrl?: string | null;
  bookmarked: boolean;
  coverUrl?: string | null;
  earlyAccess: boolean;
  emulatorId?: number | null;
  genres: string[];
  id: number;
  kind?: string;
  lastPlayedAt?: string | null;
  matchScore?: number | null;
  matchedField?: string | null;
  matchedTitle?: string | null;
  maxPlayers?: number | null;
  minutesPlayed: number;
  missingVersionCount?: number;
  platform?: string | null;
  rating?: number | null;
  releaseDate?: string | null;
  sortTitle: string;
  state: ProgressState;
  status?: string;
  title: string;
  totalSize: number;
  versionCount: number;
}

export interface GameVersion {
  archiveFormat?: string | null;
  archiveHash?: string | null;
  contentSha256?: string | null;
  downloadUrl: string;
  earlyAccess: boolean;
  fileName: string;
  id: number;
  missing?: boolean;
  releaseDate?: string | null;
  rootUnavailable?: boolean;
  size: number;
  streamSupported?: boolean;
  updatedAt?: string | null;
  version?: string | null;
}

export interface GameVersionInfo {
  archiveFormat?: string | null;
  archiveHash?: string | null;
  contentSha256?: string | null;
  downloadUrl: string;
  earlyAccess: boolean;
  fileName: string;
  id: number;
  indexedAt?: string | null;
  missing: boolean;
  releaseDate?: string | null;
  rootUnavailable: boolean;
  size: number;
  streamSupported?: boolean;
  updatedAt?: string | null;
  version?: string | null;
}

export interface GameVersionsResponse {
  defaultVersionId?: number | null;
  gameId: number;
  versions: GameVersionInfo[];
}

export interface HealthDatabase {
  backend: string;
  connections: number;
  idleConnections: number;
  maxConnections: number;
  ok: boolean;
  sizeBytes: number;
}

export interface HealthDetails {
  commit: string;
  database: HealthDatabase;
  disk: HealthDisk;
  ingest: HealthIngest;
  integrity?: IntegritySummary | null;
  library: HealthLibrary;
  status: string;
  storageBackend: string;
  uptimeSeconds: number;
  version: string;
  watcher: HealthWatcher;
}

export interface HealthDisk {
  dataFreeBytes: number;
  gamesFreeBytes: number;
  mediaFreeBytes: number;
}

export interface HealthIngest {
  cancelled: number;
  done: number;
  failed: number;
  queued: number;
  running: number;
  total: number;
}

export interface HealthLibrary {
  games: number;
  mediaBytes: number;
  mediaFiles: number;
  rootsAvailable: number;
  rootsTotal: number;
  versions: number;
}

export interface HealthResponse {
  status: string;
  uptimeSeconds: number;
  version: string;
}

export interface HealthWatcher {
  enabled: boolean;
  lastError?: string | null;
  lastEventAt?: string | null;
  pending: boolean;
  roots: number;
}

export interface ImportCollectionRequest {
  name?: string | null;
  password?: string | null;
  sourceId?: number | null;
  token?: string | null;
}

export interface ImportCollectionResponse {
  added: number;
  collection: Collection;
  created: boolean;
  skipped: number;
}

export interface IngestFile {
  modifiedAt?: string | null;
  name: string;
  partial: boolean;
  size: number;
}

export interface IngestJob {
  attempt: number;
  createdAt: string;
  downloadedBytes: number;
  error?: string | null;
  fileName?: string | null;
  id: string;
  speedBps: number;
  status: IngestStatus;
  title: string;
  totalBytes?: number | null;
  updatedAt: string;
  url?: string | null;
}

export interface IngestStats {
  activeJobs: number;
  diskFree: number;
  diskTotal: number;
  gameCount: number;
  gamesDir: string;
  libraryBytes: number;
}

export type IngestStatus = "queued" | "downloading" | "verifying" | "done" | "error" | "cancelled";

export interface InstalledReportResponse {
  recorded: boolean;
}

export interface IntegrationStatus {
  addedAt?: string | null;
  configured: boolean;
  fingerprint?: string | null;
  lastError?: string | null;
  lastUsedAt?: string | null;
  quota: QuotaStatus;
  source: KeySource;
}

export interface IntegrityCheckRequest {
  scope?: string | null;
}

export interface IntegrityCheckResponse {
  reportId: number;
  status: string;
}

export interface IntegrityFinding {
  createdAt: string;
  detail?: string | null;
  id: number;
  kind: string;
  path?: string | null;
  targetId?: string | null;
  targetType: string;
}

export interface IntegrityReport {
  checked: number;
  error?: string | null;
  findingsCount: number;
  finishedAt?: string | null;
  id: number;
  scope: string;
  startedAt: string;
  status: string;
}

export interface IntegrityReportDetail {
  findings: IntegrityFinding[];
  report: IntegrityReport;
}

export interface IntegritySummary {
  findings: number;
  finishedAt?: string | null;
  id: number;
  startedAt?: string | null;
  status: string;
}

export interface Invitation {
  createdAt: string;
  createdBy?: number | null;
  createdByUsername?: string | null;
  email?: string | null;
  expiresAt: string;
  id: number;
  lastUsedAt?: string | null;
  maxUses: number;
  note?: string | null;
  path: string;
  revokedAt?: string | null;
  role: Role;
  status: InvitationStatus;
  uses: number;
}

export interface InvitationPreview {
  email?: string | null;
  expiresAt: string;
  note?: string | null;
  requiresPassword: boolean;
  role: Role;
}

export type InvitationStatus = "pending" | "used" | "expired" | "revoked";

export type KeySource = "env" | "database" | "none";

export interface LatestVersion {
  archiveHash?: string | null;
  archiveSize: number;
  contentSha256?: string | null;
  downloadUrl: string;
  earlyAccess: boolean;
  fileName: string;
  gameId: number;
  updatedAt?: string | null;
  versionId: number;
  versionLabel?: string | null;
}

export interface LaunchInfo {
  executable?: string | null;
  installerExecutable?: string | null;
  installerParameters?: string | null;
  parameters?: string | null;
  uninstallerExecutable?: string | null;
  uninstallerParameters?: string | null;
}

export interface LibraryFolders {
  dataDir: string;
  gamesDir: string;
  mediaDir: string;
}

export interface LibraryHealthActionRequest {
  action: string;
  confirm?: boolean;
  findingIds?: number[];
}

export interface LibraryHealthActionResult {
  action: string;
  applied: number;
  errors: string[];
  mediaGc?: LibraryHealthMediaGc | null;
  reportQueued: boolean;
  skipped: number;
}

export interface LibraryHealthCategoryCount {
  category: string;
  count: number;
}

export interface LibraryHealthCounts {
  error: number;
  info: number;
  total: number;
  warning: number;
}

export interface LibraryHealthFinding {
  actions: string[];
  category: string;
  createdAt: string;
  detail: string;
  evidence: Record<string, unknown>;
  gameId?: number | null;
  id: number;
  kind: string;
  path?: string | null;
  reportId: number;
  rootId?: number | null;
  severity: string;
  title: string;
  versionId?: number | null;
}

export interface LibraryHealthMediaGc {
  candidates: string[];
  deletedBytes: number;
  deletedFiles: number;
  dryRun: boolean;
  orphanBytes: number;
  orphanFiles: number;
  referenced: number;
  scanned: number;
}

export interface LibraryHealthReport {
  byCategory: LibraryHealthCategoryCount[];
  counts: LibraryHealthCounts;
  error?: string | null;
  findingsCount: number;
  finishedAt?: string | null;
  id: number;
  progress: number;
  startedAt: string;
  status: string;
  total: number;
  trigger: string;
}

export interface LibraryHealthResponse {
  findings: LibraryHealthFinding[];
  findingsTotal: number;
  report?: LibraryHealthReport | null;
}

export interface LibraryHealthStartResponse {
  reportId: number;
  status: string;
}

export interface LibraryRootView {
  available: boolean;
  createdAt: string;
  enabled: boolean;
  id: number;
  kind: string;
  label: string;
  lastError?: string | null;
  lastScanAt?: string | null;
  path: string;
  position: number;
  versionCount: number;
}

export interface LocalPathResponse {
  path: string;
}

export interface LoginRequest {
  password: string;
  username: string;
}

export interface ManifestResponse {
  archiveSha256: string;
  directories: string[];
  fileCount: number;
  files: ExtractedFile[];
  generatedAt: string;
  status: string;
  totalBytes: number;
  versionId: number;
}

export interface MetadataCandidate {
  confidence: number;
  coverUrl?: string | null;
  developers: string[];
  platforms: string[];
  provider: string;
  providerId: string;
  providerName: string;
  providerUrl?: string | null;
  score: number;
  selected: boolean;
  summary?: string | null;
  title: string;
  year?: number | null;
}

export interface MetadataCandidatesResponse {
  candidates: MetadataCandidate[];
  gameId: number;
  manualFields: string[];
  providers: MetadataProviderStatus[];
  query: string;
}

export interface MetadataProviderStatus {
  candidates: number;
  elapsedMs: number;
  error?: string | null;
  name: string;
  ok: boolean;
  slug: string;
}

export interface MetadataProviderView {
  baseUrl: string;
  createdAt: string;
  enabled: boolean;
  id: number;
  mapping: Record<string, unknown>;
  name: string;
  priority: number;
  secretConfigured: boolean;
  updatedAt: string;
}

export interface MfaDisableRequest {
  password: string;
}

export interface MfaLoginRequest {
  challengeToken: string;
  code: string;
}

export interface MfaStatus {
  enabled: boolean;
  enabledAt?: string | null;
  methods: string[];
  pending: boolean;
  recoveryCodesRemaining: number;
}

export interface NewsPost {
  author: string;
  body: string;
  createdAt: string;
  id: number;
  title: string;
  updatedAt: string;
}

export interface Notification {
  achievement?: NotificationAchievement | null;
  actor?: SocialUser | null;
  createdAt: string;
  game?: FriendGame | null;
  id: number;
  kind: string;
  payload: Record<string, unknown>;
  readAt?: string | null;
}

export interface NotificationAchievement {
  id: number;
  key: string;
  name: string;
  points: number;
}

export interface NotificationPage {
  items: Notification[];
  nextBefore?: number | null;
  unreadCount: number;
}

export interface NotificationPrefs {
  collectionShares: boolean;
  friendAccepted: boolean;
  friendAchievements: boolean;
  friendRequests: boolean;
}

export interface NotificationPrefsPatch {
  collectionShares?: boolean | null;
  friendAccepted?: boolean | null;
  friendAchievements?: boolean | null;
  friendRequests?: boolean | null;
}

export interface NotificationReadResponse {
  unreadCount: number;
  updated: number;
}

export interface ObservabilityActive {
  downloads: number;
  ingestQueued: number;
  ingestRunning: number;
  integrityRunning: boolean;
  scans: number;
}

export interface ObservabilityBackups {
  archives: number;
  bytes: number;
  latestAt?: string | null;
}

export interface ObservabilityDatabase {
  backend: string;
  connections: number;
  idleConnections: number;
  latencyMs: number;
  maxConnections: number;
  ok: boolean;
  sizeBytes: number;
}

export interface ObservabilityDisk {
  dataFreeBytes: number;
  gamesFreeBytes: number;
  mediaFreeBytes: number;
}

export interface ObservabilityErrorRoute {
  count: number;
  method: string;
  route: string;
  status: number;
}

export interface ObservabilityEvent {
  action: string;
  actorKind: string;
  createdAt?: string | null;
  id: number;
  targetId?: string | null;
  targetType?: string | null;
}

export interface ObservabilityFinding {
  createdAt?: string | null;
  detail?: string | null;
  kind: string;
  targetId?: string | null;
  targetType: string;
}

export interface ObservabilityIntegrity {
  last?: IntegritySummary | null;
  recentFindings: ObservabilityFinding[];
}

export interface ObservabilityMedia {
  bytes: number;
  files: number;
}

export interface ObservabilityPoint {
  at: string;
  averageMs: number;
  clientErrors: number;
  redirects: number;
  requests: number;
  serverErrors: number;
  success: number;
}

export interface ObservabilityPrometheus {
  endpoint: string;
  tokenProtected: boolean;
}

export interface ObservabilityRequests {
  averageMs: number;
  clientErrors: number;
  errorRate: number;
  p50Ms: number;
  p95Ms: number;
  p99Ms: number;
  perSecond: number;
  redirects: number;
  serverErrors: number;
  success: number;
  total: number;
}

export interface ObservabilityRoot {
  available: boolean;
  enabled: boolean;
  label: string;
  lastError?: string | null;
  versions: number;
}

export interface ObservabilitySaves {
  bytes: number;
  snapshots: number;
}

export interface ObservabilityServer {
  commit: string;
  startedAt: string;
  uptimeSeconds: number;
  version: string;
}

export interface ObservabilitySnapshot {
  active: ObservabilityActive;
  database: ObservabilityDatabase;
  generatedAt: string;
  integrity: ObservabilityIntegrity;
  prometheus: ObservabilityPrometheus;
  recentEvents: ObservabilityEvent[];
  requests: ObservabilityRequests;
  series: ObservabilityPoint[];
  server: ObservabilityServer;
  storage: ObservabilityStorage;
  topErrors: ObservabilityErrorRoute[];
  webhooks: ObservabilityWebhooks;
  window: string;
}

export interface ObservabilityStorage {
  backend: string;
  backups: ObservabilityBackups;
  disk: ObservabilityDisk;
  media: ObservabilityMedia;
  roots: ObservabilityRoot[];
  saves: ObservabilitySaves;
}

export interface ObservabilityWebhookFailure {
  attempts: number;
  event: string;
  id: number;
  lastError?: string | null;
  nextAttemptAt?: string | null;
  statusCode?: number | null;
}

export interface ObservabilityWebhooks {
  failed: number;
  pending: number;
  recentFailures: ObservabilityWebhookFailure[];
}

export interface OidcExchangeRequest {
  code: string;
}

export interface OidcSettingsRequest {
  adminGroups?: string[] | null;
  autoCreate?: boolean | null;
  clearSecret?: boolean;
  clientId?: string | null;
  defaultRole?: Role | null;
  displayName?: string | null;
  enabled?: boolean | null;
  groupsClaim?: string | null;
  issuer?: string | null;
  publicBaseUrl?: string | null;
  redirectUrl?: string | null;
  replaceSecret?: string | null;
  scopes?: string | null;
}

export interface OnboardingRequest {
  completed?: boolean | null;
  step?: number | null;
}

export interface PatchPreferencesRequest {
  patch: Record<string, unknown>;
  schemaVersion?: number | null;
}

export interface PlatformDefault {
  emulatorId?: number | null;
  platform: string;
}

export interface PlatformView {
  extensions: string[];
  gameCount: number;
  id: string;
  label: string;
  retroarchCore?: string | null;
}

export interface PlaytimeRequest {
  messageId?: string | null;
  minutes: number;
}

export interface PreferencesResponse {
  document?: Record<string, unknown>;
  revision: number;
  schemaVersion: number;
  updatedAt?: string | null;
}

export interface PresenceHeartbeat {
  gameId?: number | null;
  state?: string | null;
}

export interface PresenceStateResponse {
  enabled: boolean;
  game?: FriendGame | null;
  lastSeen?: string | null;
  staleAfterSeconds: number;
  state?: string | null;
}

export interface PresetEntry {
  id: string;
  label: string;
  profile: ControllerProfile;
}

export interface ProgressRequest {
  state: ProgressState;
}

export type ProgressState = "unplayed" | "playing" | "completed" | "abandoned" | "onHold" | "planToPlay";

export interface ProgressUpdate {
  lastPlayedAt?: string | null;
  minutesPlayed: number;
  state: ProgressState;
}

export interface ProviderCandidate {
  ageRating?: number | null;
  backgroundUrl?: string | null;
  coverUrl?: string | null;
  description?: string | null;
  developers?: string[];
  genres?: string[];
  id: string;
  providerUrl?: string | null;
  publishers?: string[];
  rating?: number | null;
  releaseDate?: string | null;
  screenshots?: string[];
  tags?: string[];
  title: string;
}

export interface ProviderRef {
  id: string;
  slug: string;
  url?: string | null;
}

export interface ProviderReport {
  found: number;
  message?: string | null;
  provider: string;
  status: string;
  unlocked: number;
}

export interface ProviderTestResponse {
  candidates: ProviderCandidate[];
  elapsedMs: number;
  error?: string | null;
  ok: boolean;
  statusCode?: number | null;
}

export interface PublicProfile {
  avatarUrl?: string | null;
  bio?: string | null;
  createdAt: string;
  displayName?: string | null;
  friendsActivity: boolean;
  id: number;
  presenceEnabled: boolean;
  publicProfile: boolean;
  username: string;
}

export interface PublicProfileResponse {
  collections: Collection[];
  library: GameSummary[];
  stats: PublicStats;
  user: PublicProfile;
}

export interface PublicStats {
  bookmarked: number;
  collections: number;
  completed: number;
  games: number;
  minutesPlayed: number;
}

export interface QuotaStatus {
  dailyLimit: number;
  remaining: number;
  resetAt: string;
  usedToday: number;
}

export interface Recorded {
  recorded: boolean;
}

export interface RecoveryCodes {
  codes: string[];
}

export interface RecoveryCodesRequest {
  password: string;
}

export interface RefreshResponse {
  queued: boolean;
}

export interface RemoteInstall {
  claimedAt?: string | null;
  claimedByDeviceId?: string | null;
  completedAt?: string | null;
  coverUrl?: string | null;
  device?: string | null;
  gameId: number;
  gameTitle: string;
  id: string;
  message?: string | null;
  progress: number;
  requestedAt: string;
  sizeBytes?: number | null;
  status: RemoteInstallStatus;
  targetDeviceId?: string | null;
  targetDeviceName?: string | null;
  updatedAt: string;
  version?: string | null;
  versionId?: number | null;
}

export type RemoteInstallStatus = "queued" | "claimed" | "downloading" | "ready" | "failed" | "cancelled";

export interface RemoteInstallStatusRequest {
  clientId?: string | null;
  message?: string | null;
  progress?: number | null;
  status: RemoteInstallStatus;
}

export interface ReorderMetadataProvidersRequest {
  ids: number[];
}

export interface ReplacePreferencesRequest {
  document: Record<string, unknown>;
  schemaVersion?: number | null;
}

export interface RescanPolicyInput {
  enabled: boolean;
  includeMalicious?: boolean;
  intervalHours: number;
}

export interface RescanStatus {
  cursor?: string | null;
  due: boolean;
  enabled: boolean;
  includeMalicious: boolean;
  intervalHours: number;
  lastError?: string | null;
  lastRunAt?: string | null;
  nextRunAt?: string | null;
  remaining: number;
  scannedLastRun: number;
}

export interface ResetPasswordRequest {
  password?: string | null;
  requireChange?: boolean | null;
}

export interface ResetPasswordResult {
  mustChangePassword: boolean;
  password?: string | null;
  revoked: number;
  username: string;
}

export interface RestoreBackupRequest {
  confirmation?: string | null;
  dryRun?: boolean;
  includeMedia?: boolean;
}

export interface RestoreBackupResponse {
  bytes: number;
  createdAt: string;
  dryRun: boolean;
  files: number;
  games: number;
  overwrote: boolean;
  restartRequired: boolean;
  safetyBackup?: string | null;
  users: number;
  versions: number;
}

export type Role = "admin" | "editor" | "user";

export interface RootScanResponse {
  rootId: number;
  started: boolean;
}

export interface SaveControllerProfileRequest {
  gameId?: number | null;
  profile: ControllerProfile;
}

export interface SaveSnapshot {
  createdAt: string;
  deviceId?: string | null;
  fileName: string;
  gameId: number;
  id: number;
  label: string;
  sha256: string;
  size: number;
  source: string;
  versionId?: number | null;
}

export interface SaveVirustotalKeyRequest {
  apiKey: string;
}

export type ScanMode = "hash" | "hash_and_binaries" | "upload";

export interface ScanReport {
  checkedAt: string;
  permalink?: string | null;
  sha256: string;
  source: ScanSource;
  stats: VtStats;
  verdict: Verdict;
  versionId: number;
}

export interface ScanRequest {
  confirmUpload?: boolean;
  consent?: string | null;
  force?: boolean;
  gameId?: number | null;
  iUnderstand?: string | null;
  mode?: string | null;
  versionId?: number | null;
}

export interface ScanResponse {
  scans: ScanReport[];
}

export type ScanSource = "cache" | "api";

export interface ScanStatus {
  added: number;
  errors: string[];
  finishedAt?: string | null;
  metadataPending: number;
  removed: number;
  running: boolean;
  scanned: number;
  startedAt?: string | null;
  updated: number;
}

export interface ScanSummary {
  found: number;
  providers: string[];
  scannedAt: string;
  unlocked: number;
}

export interface ServerConfig {
  defaultLocale: string;
  features: FeatureFlags;
  maxUploadBytes: number;
  oidcDisplayName: string;
  parallelPartsMax: number;
  serverName: string;
  version: string;
}

export interface ServerSettings {
  defaultLocale: string;
  serverName: string;
}

export interface SessionRevocation {
  revoked: number;
}

export interface SetDefaultVersionRequest {
  versionId: number;
}

export interface SetGameEmulatorRequest {
  emulatorId?: number | null;
}

export interface SetupRequest {
  password: string;
  serverName?: string | null;
  username: string;
}

export interface SetupStatus {
  needsSetup: boolean;
  serverName: string;
  version: string;
}

export interface ShareAccessRequest {
  password?: string | null;
}

export interface ShareAccessResponse {
  expiresAt: string;
  url: string;
}

export interface ShareGame {
  ageRating?: number | null;
  backgroundUrl?: string | null;
  coverUrl?: string | null;
  description?: string | null;
  developers: string[];
  genres: string[];
  id: number;
  publishers: string[];
  rating?: number | null;
  releaseDate?: string | null;
  screenshots: string[];
  title: string;
}

export interface ShareLink {
  createdAt: string;
  downloadCount: number;
  expiresAt?: string | null;
  gameId: number;
  gameTitle?: string | null;
  hasPassword: boolean;
  id: number;
  label?: string | null;
  maxDownloads?: number | null;
  revokedAt?: string | null;
  token?: string | null;
  versionId?: number | null;
}

export interface ShareVersion {
  earlyAccess: boolean;
  id: number;
  releaseDate?: string | null;
  size: number;
  version?: string | null;
}

export interface ShareView {
  expiresAt?: string | null;
  game: ShareGame;
  hasPassword: boolean;
  remainingDownloads?: number | null;
  version: ShareVersion;
}

export interface SharedCollection {
  description?: string | null;
  gameCount: number;
  kind: CollectionKind;
  name: string;
  owner?: SharedCollectionOwner | null;
  updatedAt: string;
}

export interface SharedCollectionGame {
  coverUrl?: string | null;
  genres: string[];
  id: number;
  releaseYear?: number | null;
  title: string;
}

export interface SharedCollectionOwner {
  displayName: string;
}

export interface SmartRule {
  field: string;
  op?: string | null;
  value: Record<string, unknown>;
  value2?: Record<string, unknown>;
}

export interface SmartRules {
  match?: string;
  rules?: SmartRule[];
}

export interface SocialUser {
  avatarUrl?: string | null;
  displayName?: string | null;
  id: number;
  username: string;
}

export interface Stats {
  byState: Record<string, number>;
  games: number;
  totalBytes: number;
  users: number;
  versions: number;
}

export interface StreamEntry {
  compressedSize: number;
  crc32: number;
  index: number;
  isDir: boolean;
  mode: number;
  path: string;
  size: number;
}

export interface StreamManifest {
  entries: StreamEntry[];
  entryCount: number;
  format: string;
  totalBytes: number;
  totalCompressedBytes: number;
}

export interface SuggestResponse {
  query: string;
  suggestions: GameSuggestion[];
}

export interface TestMetadataProviderRequest {
  title: string;
  year?: number | null;
}

export interface TestResponse {
  message: string;
  ok: boolean;
}

export interface TicketRequest {
  versionId?: number | null;
}

export interface TicketResponse {
  expiresAt: string;
  url: string;
}

export interface TotpActivated {
  mfa: MfaStatus;
  recoveryCodes: string[];
}

export interface TotpActivationRequest {
  code: string;
}

export interface TotpEnrollment {
  account: string;
  digits: number;
  issuer: string;
  otpauthUri: string;
  period: number;
  secret: string;
}

export interface Trailer {
  provider?: string | null;
  thumbnailUrl?: string | null;
  title: string;
  url: string;
  videoId?: string | null;
}

export interface UnlockRequest {
  source?: string | null;
  userId?: number | null;
}

export interface UnlockResponse {
  achievement: Achievement;
  unlocked: boolean;
}

export interface UpdateCollectionRequest {
  description?: string | null;
  isPublic?: boolean | null;
  kind?: CollectionKind | null;
  name?: string | null;
  rules?: SmartRules | null;
  visibility?: CollectionVisibility | null;
}

export interface UpdateDefaultsRequest {
  defaults: PlatformDefault[];
}

export interface UpdateDownloadRequest {
  url: string;
}

export interface UpdateEmulatorRequest {
  command?: string | null;
  enabled?: boolean | null;
  name?: string | null;
  platforms?: string[] | null;
}

export interface UpdateGameRequest {
  ageRating?: number | null;
  altTitles?: string[] | null;
  description?: string | null;
  developers?: string[] | null;
  earlyAccess?: boolean | null;
  genres?: string[] | null;
  launch?: LaunchInfo | null;
  players?: number | null;
  publishers?: string[] | null;
  rating?: number | null;
  releaseDate?: string | null;
  sortTitle?: string | null;
  tags?: string[] | null;
  title?: string | null;
}

export interface UpdateLibraryRootRequest {
  enabled?: boolean | null;
  kind?: string | null;
  label?: string | null;
  path?: string | null;
  position?: number | null;
}

export interface UpdateMetadataProviderRequest {
  baseUrl?: string | null;
  clearToken?: boolean;
  enabled?: boolean | null;
  mapping?: Record<string, unknown>;
  name?: string | null;
  priority?: number | null;
  token?: string | null;
}

export interface UpdateNewsRequest {
  body?: string | null;
  title?: string | null;
}

export interface UpdateProfileRequest {
  bio?: string | null;
  displayName?: string | null;
  friendsActivity?: boolean | null;
  presenceEnabled?: boolean | null;
  publicProfile?: boolean | null;
}

export interface UpdateServerSettingsRequest {
  defaultLocale?: string | null;
}

export interface UpdateUserRequest {
  activated?: boolean | null;
  ageLimit?: number | null;
  email?: string | null;
  password?: string | null;
  role?: Role | null;
  username?: string | null;
}

export interface UpdateWebhookRequest {
  enabled?: boolean | null;
  events?: string[] | null;
  rotateSecret?: boolean | null;
  url?: string | null;
}

export type UpdateWebhookResponse = Webhook & {
  secret?: string | null;
};

export interface UploadPartResponse {
  id: string;
  part: number;
  receivedBytes: number;
  receivedParts: number;
  totalParts: number;
}

export interface UploadSession {
  chunkSize: number;
  createdAt: string;
  error?: string | null;
  fileName: string;
  gameId?: number | null;
  id: string;
  receivedBytes: number;
  receivedParts: number[];
  sha256?: string | null;
  size: number;
  status: string;
  totalParts: number;
  updatedAt: string;
  versionId?: number | null;
}

export interface User {
  activated: boolean;
  ageLimit?: number | null;
  avatarUrl?: string | null;
  createdAt: string;
  email?: string | null;
  id: number;
  locale?: string | null;
  mfaEnabled?: boolean;
  mustChangePassword?: boolean;
  oidcIssuer?: string | null;
  onboardingCompletedAt?: string | null;
  onboardingStep: number;
  role: Role;
  source: UserSource;
  username: string;
}

export interface UserAchievements {
  achievements: Achievement[];
  games: number;
  unlocked: number;
  unlockedPoints: number;
  user: AchievementUser;
}

export interface UserResponse {
  user: User;
}

export type UserSource = "local" | "oidc";

export interface ValidateCommandRequest {
  command: string;
}

export interface ValidateCommandResponse {
  args: string[];
  error?: string | null;
  program?: string | null;
  valid: boolean;
}

export type Verdict = "clean" | "suspicious" | "malicious" | "not_found" | "unknown" | "disabled" | "quota_exceeded" | "error";

export interface VerifyResponse {
  log: string;
  ok: boolean;
}

export interface VtStats {
  failure: number;
  harmless: number;
  malicious: number;
  suspicious: number;
  timeout: number;
  totalEngines: number;
  undetected: number;
}

export interface Webhook {
  createdAt: string;
  enabled: boolean;
  events: string[];
  id: number;
  url: string;
}

export interface WebhookTestResponse {
  deliveryId: number;
}
