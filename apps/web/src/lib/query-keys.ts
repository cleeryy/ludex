/**
 * TanStack Query keys (#237).
 *
 * One module names every cache entry so an invalidation cannot miss a variant
 * that a hand-written array elsewhere forgot. Shapes are unchanged: these keys
 * are the exact arrays the call sites used before the factory existed, which is
 * what keeps deployed caches and optimistic updates working. Singleton keys are
 * plain tuples; families expose their prefix (`root`) plus one builder per
 * shape.
 *
 * Prefixes are still per-domain (`["games"]`, `["game"]`, `["installed"]`), so
 * `invalidateQueries({ queryKey: qk.games.root })` keeps matching every list
 * variant. A new variant belongs here, next to the builder that names it.
 */

type QueryKey = readonly unknown[];

const key = (...parts: readonly unknown[]): QueryKey => parts;

export const qk = {
  /* ------------------------------- library ------------------------------- */

  games: {
    root: ["games"] as const,
    /** The unpaginated library (`limit: 200`) used by Home, the tray and the palette. */
    all: () => key("games", "all"),
    /** Infinite pages of the library (`useLibraryGames`, #267). */
    pages: () => key("games", "all", "pages"),
    /** One library tab with its parsed filters (`LibraryScreen`). */
    list: (mode: string, query: unknown) => key("games", mode, query),
    /** Metadata rows hidden by the default availability filter (#210). */
    missingCount: ["games-missing-count"] as const,
  },

  game: {
    root: ["game"] as const,
    detail: (gameId: number | undefined) => key("game", gameId),
    versions: {
      forGame: (gameId: number) => key("game-versions", gameId),
    },
  },

  installed: ["installed"] as const,

  recentGames: {
    root: ["recent-games"] as const,
    byLimit: (limit: number) => key("recent-games", limit),
  },

  gameUpdates: ["game-updates"] as const,
  playtime: {
    forGame: (gameId: number) => key("playtime", gameId),
  },
  gameSession: {
    root: ["game-session"] as const,
    forGame: (gameId: number) => key("game-session", gameId),
  },
  gameExecutables: {
    root: ["game-executables"] as const,
    forGame: (gameId: number) => key("game-executables", gameId),
  },

  facets: {
    genres: ["facets", "genres"] as const,
  },
  platforms: ["platforms"] as const,
  emulators: ["emulators"] as const,
  suggest: (query: string) => key("suggest", query),
  stats: ["stats"] as const,
  config: ["config"] as const,
  health: ["health"] as const,
  folders: ["folders"] as const,
  scan: ["scan"] as const,
  servers: ["servers"] as const,
  news: ["news"] as const,

  /* ------------------------------- account ------------------------------- */

  myProfile: ["my-profile"] as const,
  notificationPrefs: ["notification-prefs"] as const,
  notifications: {
    root: ["notifications"] as const,
    bell: () => key("notifications", "bell"),
  },
  mfa: ["mfa"] as const,
  sessions: ["sessions"] as const,
  presenceStatus: ["presence-status"] as const,
  invitation: (token: string | undefined) => key("invitation", token),

  /* ------------------------------- community ----------------------------- */

  community: {
    users: ["community-users"] as const,
    user: (userId: number | undefined) => key("community-user", userId),
    userActivity: (userId: number) => key("community-user-activity", userId),
    feed: ["community-feed"] as const,
  },
  friends: {
    root: ["friends"] as const,
    presence: ["friends-presence"] as const,
    activity: ["friends-activity"] as const,
  },
  achievements: {
    me: ["me-achievements"] as const,
    forGame: (gameId: number) => key("game-achievements", gameId),
    forUser: (userId: number) => key("user-achievements", userId),
  },

  /* ------------------------------ collections ---------------------------- */

  collections: {
    root: ["collections"] as const,
    detail: (collectionId: number | undefined) => key("collection", collectionId),
    games: (collectionId: number | undefined) => key("collection-games", collectionId),
    /** The collection editor's game membership (`["collection", id, "games"]`). */
    editorGames: (collectionId: number | undefined) => key("collection", collectionId, "games"),
  },
  collectionShares: {
    forCollection: (collectionId: number) => key("collection-shares", collectionId),
  },
  shares: ["shares"] as const,
  publicShare: (token: string | undefined) => key("public-share", token),
  publicCollectionShare: (token: string | undefined) => key("public-collection-share", token),

  /* ------------------------------ downloads ------------------------------ */

  desktopDownload: (platform: string | null) => key("desktop-download", platform ?? "all"),
  remoteInstalls: ["remote-installs"] as const,
  installJobs: ["install-jobs"] as const,
  installSpaceLedger: ["install-space-ledger"] as const,
  remotePlayStatus: (probePublic: boolean) => key("remote-play-status", probePublic),
  streamTarget: {
    root: ["stream-target"] as const,
    forGame: (gameId: number) => key("stream-target", gameId),
  },
  saves: {
    forGame: (gameId: number) => key("saves", gameId),
  },
  cloudSaveStatus: {
    root: ["cloud-save-status"] as const,
    forGame: (gameId: number) => key("cloud-save-status", gameId),
  },
  saveCandidates: ["save-candidates"] as const,
  ludusaviManifest: ["ludusavi-manifest"] as const,

  /* ---------------------------- launch options --------------------------- */

  protondb: (gameId: number) => key("protondb", gameId),
  compatScan: ["compat-scan"] as const,
  /** Host preflight (#285): persisted report, refreshed on demand. */
  hostPreflight: ["host-preflight"] as const,
  /** Suspend inhibitor (#288): live state, pushed on `inhibit://status`. */
  inhibitStatus: ["inhibit-status"] as const,
  launchOptions: (gameId: number) => key("launch-options", gameId),
  prefixInfo: (gameId: number) => key("prefix-info", gameId),
  makeItRun: (gameId: number) => key("make-it-run", gameId),
  /** Matched compatibility rules for one game (#301). */
  compatRules: (gameId: number) => key("compat-rules", gameId),
  virustotal: {
    forGame: (gameId: number) => key("game-virustotal", gameId),
  },

  /* ------------------------------ game config ---------------------------- */

  gameConfigs: {
    forGame: (gameId: number) => key("game-configs", gameId),
  },
  gameConfigAdapters: {
    forGame: (gameId: number) => key("game-config-adapters", gameId),
  },
  gameConfigBackups: {
    forGame: (gameId: number) => key("game-config-backups", gameId),
  },
  gameConfig: {
    forGame: (gameId: number, configId: string) => key("game-config", gameId, configId),
  },
  gameConfigCommonFields: {
    forGame: (gameId: number) => key("game-config-common-fields", gameId),
  },
  gameConfigSync: {
    forGame: (gameId: number) => key("game-config-sync", gameId),
  },

  /* -------------------------------- desktop ------------------------------ */

  desktopSettings: ["desktop-settings"] as const,
  desktopLocations: ["desktop-locations"] as const,
  desktopStorageSuggestions: ["desktop-storage-suggestions"] as const,
  titleBarInfo: ["title-bar-info"] as const,
  controllerProfiles: ["controller-profiles"] as const,
  devices: ["devices"] as const,
  romSettings: ["rom-settings"] as const,
  diagnostics: {
    status: ["diagnostics-status"] as const,
    admin: ["admin-diagnostics"] as const,
  },
  sunshine: {
    status: ["sunshine-status"] as const,
    previewRoot: ["sunshine-preview"] as const,
    preview: (appsPath: string) => key("sunshine-preview", appsPath),
  },
  streamHost: {
    status: ["stream-host-status"] as const,
    selfTest: ["stream-host-self-test"] as const,
    firewall: ["stream-host-firewall"] as const,
    configPlan: ["stream-host-config-plan"] as const,
  },
  moonlightStatus: ["moonlight-status"] as const,

  overlay: {
    status: ["overlay-status"] as const,
    installed: ["overlay-installed"] as const,
    jobs: ["overlay-jobs"] as const,
    achievements: (gameId: number | null) => key("overlay-achievements", gameId),
  },

  /* --------------------------------- admin ------------------------------- */

  admin: {
    server: ["admin-server"] as const,
    oidc: ["admin-oidc"] as const,
    metadataProviders: ["admin-metadata-providers"] as const,
    virusTotal: ["admin-virustotal"] as const,
    virusTotalRescan: ["admin-virustotal-rescan"] as const,
    /** Extracted cache budget and entries (#231). */
    extractedCache: ["admin", "extracted-cache"] as const,
    /** Per-version extraction status; the same POST route reports progress. */
    extract: (versionId: number) => key("admin", "extract", versionId),
    bulkJob: (jobId: string | null) => key("admin-bulk-job", jobId),
    games: {
      root: ["admin-games"] as const,
      list: (query: unknown) => key("admin-games", query),
    },
    backups: {
      root: ["admin", "backups"] as const,
      restorePreview: (name: string) => key("admin", "backups", name, "restore-preview"),
    },
  },
  ingest: {
    jobs: ["jobs"] as const,
    files: ["files"] as const,
    stats: ["ingest-stats"] as const,
  },
  users: ["users"] as const,
  invitations: ["invitations"] as const,
  observability: (window: string) => key("observability", window),
  libraryHealth: {
    root: ["library-health"] as const,
    list: (severity: unknown, category: unknown, limit: number) =>
      key("library-health", severity, category, limit),
  },
  metadata: {
    candidates: (gameId: number, title: string, year: string, provider: string | null) =>
      key("metadata-candidates", gameId, title, year, provider ?? "*"),
  },

  /* --------------------------------- misc -------------------------------- */

  library: {
    root: ["library"] as const,
    titles: ["library-titles"] as const,
  },
  paletteGames: ["palette-games"] as const,
} as const;

export type QueryKeys = typeof qk;
