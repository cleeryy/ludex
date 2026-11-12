import {
  DEMO_ACTIVITIES,
  DEMO_COLLECTION_GAMES,
  DEMO_COLLECTIONS,
  DEMO_GAMES,
  DEMO_NEWS,
  DEMO_PASSWORD,
  DEMO_PROFILES,
  DEMO_SERVER_NAME,
  DEMO_USERS,
} from "@/data/demo";
import { hashString } from "@/lib/art";
import { ApiError, sleep } from "@/lib/http";
import { normalizeSearch, scoreSearch } from "@/lib/search";
import type {
  Achievement,
  AchievementScanReport,
  Activity,
  ActivityKind,
  ActivityPage,
  AuthResult,
  AuthSession,
  Collection,
  CollectionGamesResponse,
  CollectionInput,
  CompleteUploadResponse,
  DesktopDownload,
  DesktopHandoff,
  DesktopPlatform,
  GameAchievements,
  GameDetail,
  GameList,
  GameListQuery,
  GamePatch,
  GameSuggestion,
  GameSummary,
  IngestJob,
  IngestStats,
  InstalledGame,
  LibraryFile,
  LibraryStats,
  MetadataApplyInput,
  MetadataCandidate,
  MetadataCandidates,
  MetadataProvider,
  CreatedInvitation,
  Invitation,
  InvitationPreview,
  MfaStatus,
  MetadataProviderInput,
  NewsPost,
  OidcAdminResponse,
  OidcSettings,
  OidcSettingsInput,
  OidcTestResult,
  ProfilePatch,
  ProgressState,
  ProviderCandidate,
  ProviderTestResult,
  PublicProfile,
  PublicProfileResponse,
  RecoveryCodes,
  ResetPasswordResult,
  Role,
  SaveSnapshot,
  ScanStatus,
  ServerConfig,
  SetupStatus,
  SmartRules,
  SuggestResponse,
  TotpActivated,
  TotpEnrollment,
  UploadPartResponse,
  UploadSession,
  User,
  UserAchievements,
} from "@/lib/types";
import { evaluateRules } from "@/lib/collections";

/**
 * Mock adapter. Behaves like the real API: latency, error envelopes, filters,
 * pagination, job progress that advances, and a scanner that finishes.
 *
 * Two demo switches live in localStorage so screens can be reviewed in states
 * that are otherwise hard to reach:
 *   ludex.demo.needsSetup = "1"  -> first-run server with no users
 *   ludex.demo.empty      = "1"  -> empty library
 */

const STORAGE_SETUP = "ludex.demo.needsSetup";
const STORAGE_EMPTY = "ludex.demo.empty";
const STORAGE_USERS = "ludex.demo.users";

const GB = 1024 ** 3;
const MB = 1024 ** 2;

const latency = () => sleep(80 + Math.random() * 170);

function fail(status: number, code: string, message: string): never {
  throw new ApiError(status, code, message);
}

function nowIso() {
  return new Date().toISOString();
}

function clone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

/* --------------------------------- state --------------------------------- */

let games: GameDetail[] = clone(DEMO_GAMES);
/** Fields edited by hand in the demo, mirroring `games.manual_fields_json` (#135). */
const manualFields = new Map<number, Set<string>>();
let users: User[] = loadUsers();
let passwords = new Map<number, string>();
let news: NewsPost[] = clone(DEMO_NEWS);
let jobs: IngestJob[] = [
  {
    id: "job_a1f9",
    title: "Forza Horizon 6 (2026) v1.0.118",
    url: "https://files.example.net/Forza%20Horizon%206%20(2026)%20v1.0.118.zip",
    status: "downloading",
    fileName: "Forza Horizon 6 (2026) v1.0.118.zip",
    downloadedBytes: 9.2 * GB,
    totalBytes: 18.4 * GB,
    speedBps: 58 * MB,
    attempt: 1,
    error: null,
    createdAt: new Date(Date.now() - 1000 * 60 * 14).toISOString(),
    updatedAt: nowIso(),
  },
  {
    id: "job_b7c2",
    title: "Outer Wilds (2019) v1.1.15",
    url: "https://files.example.net/Outer%20Wilds%20(2019)%20v1.1.15.zip",
    status: "done",
    fileName: "Outer Wilds (2019) v1.1.15.zip",
    downloadedBytes: 8.7 * GB,
    totalBytes: 8.7 * GB,
    speedBps: 0,
    attempt: 1,
    error: null,
    createdAt: new Date(Date.now() - 1000 * 60 * 60 * 5).toISOString(),
    updatedAt: new Date(Date.now() - 1000 * 60 * 52).toISOString(),
  },
  {
    id: "job_c3d8",
    title: "Little Nightmares III (2025) v1.0.3",
    url: "https://files.example.net/Little%20Nightmares%20III%20(2025)%20v1.0.3.zip",
    status: "error",
    fileName: "Little Nightmares III (2025) v1.0.3.zip",
    downloadedBytes: 0,
    totalBytes: 18.5 * GB,
    speedBps: 0,
    attempt: 2,
    error: "The host closed the connection with HTTP 403",
    createdAt: new Date(Date.now() - 1000 * 60 * 60 * 26).toISOString(),
    updatedAt: new Date(Date.now() - 1000 * 60 * 60 * 25).toISOString(),
  },
];

const demoHash = (seed: string): string =>
  Array.from({ length: 8 }, (_, index) => hashString(`${seed}:${index}`).toString(16).padStart(8, "0")).join("");

let saves: SaveSnapshot[] = [
  {
    id: 1,
    gameId: 14,
    versionId: null,
    label: "auto · Living room PC",
    fileName: "outer-wilds-quantum-moon.zip",
    size: Math.round(4.2 * MB),
    sha256: demoHash("outer-wilds-quantum-moon"),
    createdAt: "2026-11-08T22:14:00Z",
    source: "auto",
    deviceId: "demo-device-01",
  },
  {
    id: 2,
    gameId: 14,
    versionId: null,
    label: "Endgame, Ash Twin",
    fileName: "outer-wilds-ash-twin.zip",
    size: Math.round(8.9 * MB),
    sha256: demoHash("outer-wilds-ash-twin"),
    createdAt: "2026-10-27T19:02:00Z",
    source: "manual",
    deviceId: null,
  },
  {
    id: 3,
    gameId: 7,
    versionId: null,
    label: "Chapter 4, before the clock",
    fileName: "it-takes-two-chapter-4.zip",
    size: Math.round(12.1 * MB),
    sha256: demoHash("it-takes-two-chapter-4"),
    createdAt: "2026-11-06T20:41:00Z",
    source: "manual",
    deviceId: null,
  },
];
let nextSaveId = 4;


/* ------------------------------ collections ------------------------------ */

let collections: Collection[] = clone(DEMO_COLLECTIONS);
let collectionMembers: Record<number, number[]> = clone(DEMO_COLLECTION_GAMES);
let profiles: PublicProfile[] = clone(DEMO_PROFILES);
let activities: Activity[] = clone(DEMO_ACTIVITIES);
let nextCollectionId = Math.max(0, ...collections.map((collection) => collection.id)) + 1;
let nextActivityId = Math.max(0, ...activities.map((activity) => activity.id)) + 1;

function collectionGameIds(collection: Collection): number[] {
  if (collection.kind === "manual") return collectionMembers[collection.id] ?? [];
  return visibleGames()
    .filter((game) => evaluateRules(collection.rules, game))
    .map((game) => game.id);
}

function visibleCollections(viewer: User): Collection[] {
  return collections
    .filter((collection) => collection.owner.id === viewer.id || collection.isPublic)
    .map((collection) => decorateCollection(collection, viewer))
    .sort((a, b) => Number(b.owned) - Number(a.owned) || a.name.localeCompare(b.name));
}

function decorateCollection(collection: Collection, viewer: User): Collection {
  return {
    ...collection,
    gameCount: collectionGameIds(collection).length,
    owned: collection.owner.id === viewer.id,
  };
}

function findCollection(id: number, viewer: User): Collection {
  const collection = collections.find((entry) => entry.id === id);
  if (!collection) fail(404, "collection_not_found", "Collection not found");
  if (collection.owner.id !== viewer.id && !collection.isPublic) {
    fail(404, "collection_not_found", "Collection not found");
  }
  return collection;
}

function ownedCollection(id: number, viewer: User): Collection {
  const collection = collections.find((entry) => entry.id === id);
  if (!collection || collection.owner.id !== viewer.id) {
    fail(404, "collection_not_found", "Collection not found");
  }
  return collection;
}

const SMART_FIELDS = ["genre", "year", "players", "playtime", "rating", "installed", "provider", "title"];

function validateRules(rules: SmartRules): void {
  if (rules.match !== "all" && rules.match !== "any") fail(400, "invalid_rules", "match must be all or any");
  for (const rule of rules.rules) {
    if (!SMART_FIELDS.includes(rule.field)) fail(400, "invalid_rules", `Unknown rule field: ${rule.field}`);
    if (rule.field === "installed" && typeof rule.value !== "boolean") {
      fail(400, "invalid_rules", "installed expects true or false");
    }
  }
}

/* ------------------------------- community ------------------------------- */

function profileFor(user: User): PublicProfile {
  let profile = profiles.find((entry) => entry.id === user.id);
  if (!profile) {
    profile = {
      id: user.id,
      username: user.username,
      displayName: null,
      bio: null,
      avatarUrl: user.avatarUrl,
      publicProfile: false,
      createdAt: user.createdAt,
    };
    profiles = [...profiles, profile];
  }
  return profile;
}

function activityUser(user: User) {
  const profile = profileFor(user);
  return {
    id: user.id,
    username: user.username,
    displayName: profile.displayName,
    avatarUrl: profile.avatarUrl,
  };
}

function recordActivity(
  user: User,
  kind: ActivityKind,
  options: { gameId?: number | null; collectionId?: number | null; metadata?: Record<string, unknown>; dedupeMinutes?: number } = {},
): void {
  const { gameId = null, collectionId = null, metadata = {}, dedupeMinutes = 0 } = options;
  if (dedupeMinutes > 0) {
    const since = Date.now() - dedupeMinutes * 60_000;
    const duplicate = activities.some(
      (activity) =>
        activity.user.id === user.id &&
        activity.kind === kind &&
        (activity.game?.id ?? null) === gameId &&
        Date.parse(activity.createdAt) >= since,
    );
    if (duplicate) return;
  }
  const game = gameId === null ? null : visibleGames().find((entry) => entry.id === gameId) ?? null;
  const collection = collectionId === null ? null : collections.find((entry) => entry.id === collectionId) ?? null;
  activities = [
    ...activities,
    {
      id: nextActivityId++,
      kind,
      createdAt: nowIso(),
      metadata,
      user: activityUser(user),
      game: game
        ? { id: game.id, title: game.title, coverUrl: game.coverUrl, releaseYear: game.releaseDate ? Number(game.releaseDate.slice(0, 4)) : null }
        : null,
      collection: collection ? { id: collection.id, name: collection.name } : null,
    },
  ];
}

function activityPage(list: Activity[], limit: number, before?: number | null): ActivityPage {
  const sorted = [...list].sort((a, b) => b.id - a.id);
  const filtered = before ? sorted.filter((activity) => activity.id < before) : sorted;
  const items = filtered.slice(0, limit);
  const nextBefore = filtered.length > limit ? items[items.length - 1]?.id ?? null : null;
  return { items, nextBefore };
}

function profileStats(userId: number, viewer: User): PublicProfileResponse["stats"] {
  const profile = profiles.find((entry) => entry.id === userId);
  if (!profile) return { games: 0, completed: 0, minutesPlayed: 0, bookmarked: 0, collections: 0 };
  const shelf = visibleGames().filter((game) => game.state !== "unplayed" || game.minutesPlayed > 0 || game.bookmarked);
  return {
    games: shelf.length,
    completed: shelf.filter((game) => game.state === "completed").length,
    minutesPlayed: shelf.reduce((total, game) => total + game.minutesPlayed, 0),
    bookmarked: shelf.filter((game) => game.bookmarked).length,
    collections: collections.filter((collection) => collection.owner.id === userId && (collection.isPublic || viewer.id === userId)).length,
  };
}

let scan: ScanStatus = {
  running: false,
  startedAt: new Date(Date.now() - 1000 * 60 * 60 * 6).toISOString(),
  finishedAt: new Date(Date.now() - 1000 * 60 * 60 * 6 + 1000 * 42).toISOString(),
  scanned: 18,
  added: 0,
  updated: 0,
  removed: 0,
  metadataPending: 0,
  errors: [],
};

let verify: { name: string; ok: boolean; log: string } | null = null;

function loadUsers(): User[] {
  const raw = localStorage.getItem(STORAGE_USERS);
  if (raw) {
    try {
      const parsed = JSON.parse(raw) as User[];
      if (Array.isArray(parsed)) {
        // Rows persisted before account sources existed are local accounts.
        return parsed.map((user) => ({
          ...user,
          source: user.source === "oidc" ? "oidc" : "local",
          oidcIssuer: user.oidcIssuer ?? null,
          onboardingCompletedAt: user.onboardingCompletedAt ?? null,
          onboardingStep: typeof user.onboardingStep === "number" ? user.onboardingStep : 0,
        }));
      }
    } catch {
      /* ignore corrupt demo state */
    }
  }
  if (localStorage.getItem(STORAGE_SETUP) === "1") return [];
  return clone(DEMO_USERS);
}

function passwordFor(user: User): string {
  return passwords.get(user.id) ?? DEMO_PASSWORD;
}

function persistUsers() {
  localStorage.setItem(STORAGE_USERS, JSON.stringify(users));
}

function emptyLibrary(): boolean {
  return localStorage.getItem(STORAGE_EMPTY) === "1";
}

function visibleGames(): GameDetail[] {
  return emptyLibrary() ? [] : games;
}

function currentUser(token: string | null): User {
  if (!token) fail(401, "unauthenticated", "Sign in to continue");
  const match = /^mock\.(\d+)\./.exec(token);
  if (!match) fail(401, "unauthenticated", "Your session expired");
  const id = Number(match[1]);
  const user = users.find((u) => u.id === id);
  if (!user) fail(401, "unauthenticated", "Your session expired");
  if (!user.activated) fail(403, "account_disabled", "This account is not activated");
  return user;
}

function summary(game: GameDetail): GameSummary {
  const {
    id,
    title,
    sortTitle,
    releaseDate,
    coverUrl,
    backgroundUrl,
    rating,
    genres,
    earlyAccess,
    versionCount,
    totalSize,
    state,
    minutesPlayed,
    bookmarked,
    lastPlayedAt,
    maxPlayers,
    kind,
    platform,
    emulatorId,
  } = game;
  return {
    id,
    title,
    sortTitle,
    releaseDate,
    coverUrl,
    backgroundUrl,
    rating,
    genres,
    earlyAccess,
    versionCount,
    totalSize,
    state,
    minutesPlayed,
    bookmarked,
    lastPlayedAt,
    maxPlayers,
    kind,
    platform,
    emulatorId,
  };
}

function filterGames(query: GameListQuery): GameSummary[] {
  const q = query.q?.trim();
  let items = visibleGames().filter((g) => {
    if (q) {
      const score = Math.max(scoreSearch(q, g.title, g.altTitles), scoreSearch(q, g.sortTitle, g.altTitles));
      if (score <= 0) return false;
    }
    if (query.genre && !g.genres.includes(query.genre)) return false;
    if (query.state && g.state !== query.state) return false;
    if (query.bookmarked && !g.bookmarked) return false;
    if (typeof query.earlyAccess === "boolean" && g.earlyAccess !== query.earlyAccess) return false;
    if (query.yearFrom && Number(g.releaseDate?.slice(0, 4) ?? 0) < query.yearFrom) return false;
    if (query.yearTo && Number(g.releaseDate?.slice(0, 4) ?? 0) > query.yearTo) return false;
    if (query.players && (g.maxPlayers ?? 0) < query.players) return false;
    if (query.playtimeMin !== undefined && g.minutesPlayed < query.playtimeMin) return false;
    if (query.playtimeMax !== undefined && g.minutesPlayed > query.playtimeMax) return false;
    if (query.minRating !== undefined && (g.rating ?? 0) < query.minRating) return false;
    if (query.provider && g.provider?.slug !== query.provider) return false;
    if (query.platform && g.platform !== query.platform) return false;
    if (query.kind && (g.kind ?? "game") !== query.kind) return false;
    return true;
  });

  if (q) {
    const normalized = normalizeSearch(q);
    const relevance = (g: GameDetail) => scoreNormalizedFor(normalized, g);
    items = items.sort((a, b) => relevance(b) - relevance(a) || a.sortTitle.localeCompare(b.sortTitle));
    return items.map((game) => {
      const found = bestMatch(q, game);
      return {
        ...summary(game),
        matchedField: found?.field,
        matchedTitle: found?.text,
        matchScore: found?.score,
      };
    });
  }

  const sort = query.sort ?? "title";
  const dir = (query.order ?? (sort === "title" ? "asc" : "desc")) === "asc" ? 1 : -1;
  const value = (g: GameDetail): string | number => {
    switch (sort) {
      case "releaseDate":
        return g.releaseDate ? Date.parse(g.releaseDate) : 0;
      case "rating":
        return g.rating ?? 0;
      case "addedAt":
        return g.id;
      case "lastPlayed":
        return g.lastPlayedAt ? Date.parse(g.lastPlayedAt) : 0;
      case "playtime":
        return g.minutesPlayed;
      default:
        return g.sortTitle;
    }
  };

  items = items.sort((a, b) => {
    const av = value(a);
    const bv = value(b);
    if (typeof av === "string" && typeof bv === "string") return av.localeCompare(bv) * dir;
    return ((av as number) - (bv as number)) * dir;
  });

  return items.map(summary);
}

function scoreNormalizedFor(normalized: string, game: GameDetail): number {
  return Math.max(
    scoreSearch(normalized, game.title, game.altTitles),
    scoreSearch(normalized, game.sortTitle, game.altTitles),
  );
}

function bestMatch(query: string, game: GameDetail): { field: "title" | "sortTitle" | "altTitle"; text: string; score: number } | null {
  const candidates: { field: "title" | "sortTitle" | "altTitle"; text: string }[] = [
    { field: "title", text: game.title },
    ...(game.sortTitle !== game.title ? [{ field: "sortTitle" as const, text: game.sortTitle }] : []),
    ...game.altTitles.map((alias) => ({ field: "altTitle" as const, text: alias })),
  ];
  let best: { field: "title" | "sortTitle" | "altTitle"; text: string; score: number } | null = null;
  for (const candidate of candidates) {
    const score = scoreSearch(query, candidate.text);
    if (score > 0 && (!best || score > best.score + 0.5)) best = { ...candidate, score };
  }
  return best;
}

/* --------------------------------- ticking -------------------------------- */

let started = false;

function startTicker() {
  if (started) return;
  started = true;
  setInterval(() => {
    for (const job of jobs) {
      if (job.status === "downloading" && job.totalBytes) {
        job.downloadedBytes = Math.min(job.totalBytes, job.downloadedBytes + job.speedBps);
        job.updatedAt = nowIso();
        if (job.downloadedBytes >= job.totalBytes) {
          job.status = "verifying";
          job.speedBps = 0;
          job.updatedAt = nowIso();
          setTimeout(() => {
            job.status = "done";
            job.error = null;
            job.updatedAt = nowIso();
          }, 3200);
        }
      }
    }

    if (scan.running) {
      scan.scanned = Math.min(scan.scanned + 9, 18);
      scan.updated = Math.min(scan.updated + 2, 4);
      if (scan.scanned >= 18) {
        scan.running = false;
        scan.finishedAt = nowIso();
        scan.metadataPending = 1;
      }
    }
  }, 1000);
}

startTicker();

/* ------------------------------ demo state ------------------------------- */

const demoUploads = new Map<string, UploadSession>();

/** Demo OIDC settings: enabled so the login page still shows the SSO button. */
const demoOidc: OidcSettings = {
  enabled: true,
  issuer: "https://auth.demo.example/application/o/ludex/",
  clientId: "ludex-demo",
  redirectUrl: null,
  publicBaseUrl: "https://ludex.demo.example",
  scopes: "openid profile email",
  displayName: "Authentik",
  autoCreate: true,
  defaultRole: "user",
  groupsClaim: "groups",
  adminGroups: ["ludex-admins"],
  secretConfigured: true,
};

function demoOidcResponse(settings: OidcSettings): OidcAdminResponse {
  return {
    settings: clone(settings),
    envLockedFields: [],
    status: {
      ok: true,
      issuer: settings.issuer,
      checkedAt: new Date().toISOString(),
      error: null,
    },
    linkedAccounts: users.filter((user) => user.oidcIssuer).length,
  };
}

function demoOidcDiscovery(issuer: string): OidcTestResult {
  const base = issuer.replace(/\/$/, "");
  return {
    ok: true,
    discovery: {
      issuer: base,
      authorizationEndpoint: `${base}/authorize`,
      tokenEndpoint: `${base}/token`,
      jwksUri: `${base}/jwks`,
      idTokenSigningAlgValuesSupported: ["RS256"],
      tokenEndpointAuthMethodsSupported: ["client_secret_basic", "client_secret_post"],
    },
  };
}

/** Demo metadata providers (#28). Tokens are never modelled in the mock. */
let demoProviders: MetadataProvider[] = [
  {
    id: 1,
    name: "ScreenScraper",
    baseUrl: "https://api.screenscraper.example/v1",
    secretConfigured: true,
    priority: 10,
    enabled: true,
    mapping: {},
    createdAt: "2026-10-02T09:12:00Z",
    updatedAt: "2026-10-02T09:12:00Z",
  },
  {
    id: 2,
    name: "GiantBomb",
    baseUrl: "https://www.giantbomb.example/api",
    secretConfigured: false,
    priority: 20,
    enabled: false,
    mapping: { title: "data.name", coverUrl: "data.image.medium_url" },
    createdAt: "2026-10-11T16:40:00Z",
    updatedAt: "2026-10-11T16:40:00Z",
  },
];

function demoProviderCandidates(title: string): ProviderCandidate[] {
  return [
    {
      id: "demo-1",
      title,
      description: `Demo metadata for ${title}.`,
      releaseDate: "2020-09-17",
      genres: ["Action"],
      developers: ["Demo Studio"],
      rating: 88,
    },
  ];
}

function requireDemoAdmin(token: string | null): User {
  const user = currentUser(token);
  if (user.role !== "admin") fail(403, "forbidden", "Administrator role required");
  return user;
}

/* ---------------------------------- api ---------------------------------- */


/* ------------------------------ achievements ----------------------------- */

const ACHIEVEMENT_SEEDS: { key: string; name: string; description: string; points: number; hidden: boolean }[] = [
  { key: "first_steps", name: "First Steps", description: "Finish the opening chapter.", points: 10, hidden: false },
  { key: "steady_hands", name: "Steady Hands", description: "Clear a section without taking damage.", points: 20, hidden: false },
  { key: "archivist", name: "Archivist", description: "Find every collectible in one area.", points: 30, hidden: false },
  { key: "against_the_clock", name: "Against the Clock", description: "Beat the par time for a full run.", points: 40, hidden: false },
  { key: "the_long_way", name: "The Long Way", description: "Take the route nobody recommends.", points: 50, hidden: true },
];

function demoAchievements(game: GameDetail, userId: number): Achievement[] {
  const owned =
    game.state === "completed" ? 4 : game.state === "playing" ? 2 : game.state === "abandoned" ? 1 : 0;
  // Other profiles get a stable but different spread so the showcase is not
  // a copy of the signed-in account.
  const unlockedCount =
    userId === 1 ? owned : Math.min(ACHIEVEMENT_SEEDS.length, (hashString(game.title) + userId) % 6);
  return ACHIEVEMENT_SEEDS.map((seed, index) => {
    const unlocked = index < unlockedCount;
    const secret = seed.hidden && !unlocked;
    return {
      id: game.id * 100 + index + 1,
      gameId: game.id,
      gameTitle: game.title,
      coverUrl: game.coverUrl,
      key: `${seed.key}_${game.id}`,
      name: secret ? "Hidden achievement" : seed.name,
      description: secret ? null : seed.description,
      iconUrl: null,
      points: seed.points,
      hidden: seed.hidden,
      provider: index % 2 === 0 ? "game_files" : "manual",
      unlocked,
      unlockedAt: unlocked
        ? new Date(Date.UTC(2026, index % 10, 3 + index + (game.id % 7), 18, 30)).toISOString()
        : null,
      progress: unlocked ? 1 : 0,
      unlockPercentage: Math.round(((hashString(`${game.title}:${seed.key}`) % 640) / 10) * 10) / 10,
      source: unlocked ? "game_files" : null,
    } satisfies Achievement;
  });
}

function demoUserAchievements(
  userId: number,
  profile: { id: number; username: string; displayName?: string | null; publicProfile?: boolean },
): UserAchievements {
  const achievements = DEMO_GAMES.flatMap((game) => demoAchievements(game, userId))
    .filter((entry) => entry.unlocked)
    .sort((left, right) => (right.unlockedAt ?? "").localeCompare(left.unlockedAt ?? ""));
  return {
    user: {
      id: profile.id,
      username: profile.username,
      displayName: profile.displayName ?? null,
      publicProfile: profile.publicProfile ?? true,
    },
    unlocked: achievements.length,
    unlockedPoints: achievements.reduce((sum, entry) => sum + entry.points, 0),
    games: new Set(achievements.map((entry) => entry.gameId)).size,
    achievements,
  };
}

/**
 * Demo candidates for the manual match dialog (#135): the current record plus
 * an IGDB and a Steam alternative, so the dialog can be reviewed in the demo.
 */
function demoCandidates(game: GameDetail): MetadataCandidate[] {
  const year = game.releaseDate ? Number(game.releaseDate.slice(0, 4)) : null;
  const currentSlug = game.provider?.slug ?? "demo";
  return [
    {
      provider: currentSlug,
      providerName: currentSlug === "demo" ? "Ludex demo" : currentSlug.toUpperCase(),
      providerId: game.provider?.id ?? `demo-${game.id}`,
      providerUrl: game.provider?.url ?? null,
      title: game.title,
      year,
      coverUrl: game.coverUrl,
      summary: game.description,
      developers: game.developers,
      platforms: ["Windows"],
      score: 1000,
      confidence: 1,
      selected: true,
    },
    {
      provider: "igdb",
      providerName: "IGDB",
      providerId: `igdb-${game.id}`,
      providerUrl: `https://www.igdb.com/games/${game.id}`,
      title: game.title,
      year: year ? year - 7 : null,
      coverUrl: game.coverUrl,
      summary: `${game.title} — demo IGDB record used by the web demo build.`,
      developers: game.developers,
      platforms: ["PC (Microsoft Windows)", "PlayStation 5"],
      score: 940,
      confidence: 0.81,
      selected: currentSlug === "igdb" && game.provider?.id === `igdb-${game.id}`,
    },
    {
      provider: "steam",
      providerName: "Steam",
      providerId: String(100_000 + game.id),
      providerUrl: `https://store.steampowered.com/app/${100_000 + game.id}`,
      title: `${game.title} Demo`,
      year,
      coverUrl: game.coverUrl,
      summary: "Steam demo record used by the web demo build.",
      developers: game.developers,
      platforms: ["Windows", "Linux"],
      score: 700,
      confidence: 0.6,
      selected: currentSlug === "steam" && game.provider?.id === String(100_000 + game.id),
    },
  ];
}

export const mockApi = {
  async setupStatus(): Promise<SetupStatus> {
    await latency();
    return { needsSetup: users.length === 0, serverName: DEMO_SERVER_NAME, version: "0.1.0" };
  },

  async setup(body: { username: string; password: string; serverName?: string }): Promise<AuthResult> {
    await latency();
    if (users.length > 0) fail(409, "setup_complete", "This server already has an administrator");
    if (body.username.trim().length < 3) fail(400, "invalid_username", "Use at least three characters");
    if (body.password.length < 8) fail(400, "weak_password", "Use at least eight characters");
    const user: User = {
      id: 1,
      username: body.username.trim(),
      email: null,
      role: "admin",
      activated: true,
      avatarUrl: null,
      createdAt: nowIso(),
      ageLimit: null,
      source: "local",
      oidcIssuer: null,
      onboardingCompletedAt: null,
      onboardingStep: 0,
      mfaEnabled: false,
      mustChangePassword: false,
    };
    users = [user];
    persistUsers();
    localStorage.removeItem(STORAGE_SETUP);
    return { token: `mock.${user.id}.${Date.now()}`, user };
  },

  async login(body: { username: string; password: string }): Promise<AuthResult> {
    await latency();
    const user = users.find((u) => u.username.toLowerCase() === body.username.trim().toLowerCase());
    if (!user || body.password !== passwordFor(user)) fail(401, "invalid_credentials", "Invalid username or password");
    if (!user.activated) fail(403, "account_disabled", "This account is not activated");
    return { token: `mock.${user.id}.${Date.now()}`, user };
  },

  async me(token: string | null): Promise<User> {
    await latency();
    return currentUser(token);
  },

  async oidcExchange(code: string): Promise<AuthResult> {
    await latency();
    if (!code) fail(400, "invalid_code", "Missing sign-in code");
    const user = users[0];
    if (!user) fail(409, "setup_required", "This server has no accounts yet");
    if (!user.activated) fail(403, "account_disabled", "This account is not activated");
    return { token: `mock.${user.id}.${Date.now()}`, user };
  },

  /** Desktop handoff (#157): the demo build accepts any well-formed state. */
  async createDesktopHandoff(token: string | null, state: string): Promise<DesktopHandoff> {
    await latency();
    currentUser(token);
    if (!state) fail(400, "invalid_handoff_state", "Missing handoff state");
    return { expiresAt: new Date(Date.now() + 120_000).toISOString() };
  },

  async changePassword(token: string | null, body: { currentPassword: string; newPassword: string }): Promise<void> {
    await latency();
    const user = currentUser(token);
    if (body.currentPassword !== passwordFor(user)) fail(400, "invalid_password", "Current password is incorrect");
    if (body.newPassword.length < 8) fail(400, "weak_password", "Use at least eight characters");
    passwords.set(user.id, body.newPassword);
  },

  async updateOnboarding(token: string | null, body: { completed?: boolean; step?: number }): Promise<User> {
    await latency();
    const user = currentUser(token);
    if (body.step !== undefined) {
      if (!Number.isInteger(body.step) || body.step < 0 || body.step > 20)
        fail(400, "invalid_onboarding_step", "Onboarding step must be between 0 and 20");
      user.onboardingStep = body.step;
    }
    if (body.completed !== undefined) {
      user.onboardingCompletedAt = body.completed ? nowIso() : null;
    }
    persistUsers();
    return clone(user);
  },

  async listGames(query: GameListQuery): Promise<GameList> {
    await latency();
    const items = filterGames(query);
    const page = query.page ?? 1;
    const limit = query.limit ?? 48;
    const start = (page - 1) * limit;
    return { items: items.slice(start, start + limit), total: items.length, page, limit };
  },

  async suggestGames(query: string, limit = 8): Promise<SuggestResponse> {
    await latency();
    const q = query.trim();
    if (!q) return { query: "", suggestions: [] };
    const scored = visibleGames()
      .map((game) => ({ game, match: bestMatch(q, game) }))
      .filter((entry) => entry.match !== null)
      .sort((a, b) => (b.match?.score ?? 0) - (a.match?.score ?? 0) || a.game.sortTitle.localeCompare(b.game.sortTitle))
      .slice(0, limit);
    const suggestions: GameSuggestion[] = scored.map(({ game, match }) => ({
      id: game.id,
      title: game.title,
      year: game.releaseDate ? Number(game.releaseDate.slice(0, 4)) : null,
      coverUrl: game.coverUrl,
      matchedField: match?.field ?? "title",
      matchedTitle: match?.text ?? game.title,
      score: match?.score ?? 0,
    }));
    return { query: q, suggestions };
  },

  async getGame(id: number): Promise<GameDetail> {
    await latency();
    const game = visibleGames().find((g) => g.id === id);
    if (!game) fail(404, "game_not_found", "That game is not in the library");
    return clone(game);
  },

  async randomGame(query: GameListQuery): Promise<GameSummary> {
    await latency();
    const items = filterGames(query);
    if (!items.length) fail(404, "library_empty", "No games match those filters");
    return items[Math.floor(Math.random() * items.length)];
  },

  async patchGame(token: string | null, id: number, patch: GamePatch): Promise<GameDetail> {
    await latency();
    const user = currentUser(token);
    if (user.role === "user") fail(403, "forbidden", "Editor role required");
    const game = games.find((g) => g.id === id);
    if (!game) fail(404, "game_not_found", "That game is not in the library");
    const { players, launch, ...rest } = patch;
    Object.assign(game, rest);
    if (players !== undefined) game.maxPlayers = players;
    if (launch) game.launch = { ...game.launch, ...launch };
    if (patch.title) game.sortTitle = patch.title.toLowerCase();
    // #135: remember hand edits so a later provider match can preserve them.
    const marked = manualFields.get(id) ?? new Set<string>();
    const tracked: Array<[keyof GamePatch, string]> = [
      ["title", "title"],
      ["releaseDate", "releaseDate"],
      ["description", "description"],
      ["genres", "genres"],
      ["developers", "developers"],
      ["publishers", "publishers"],
      ["rating", "rating"],
      ["ageRating", "ageRating"],
      ["altTitles", "altTitles"],
      ["players", "players"],
    ];
    for (const [key, field] of tracked) {
      if (patch[key] !== undefined) marked.add(field);
    }
    if (marked.size) manualFields.set(id, marked);
    return clone(game);
  },

  async refreshMetadata(token: string | null, id: number): Promise<{ queued: true }> {
    await latency();
    const user = currentUser(token);
    if (user.role === "user") fail(403, "forbidden", "Editor role required");
    if (!games.some((g) => g.id === id)) fail(404, "game_not_found", "That game is not in the library");
    return { queued: true };
  },

  /** Manual metadata match (#135): demo candidates derived from the game. */
  async metadataCandidates(
    token: string | null,
    id: number,
    params: { q?: string; provider?: string; year?: number },
  ): Promise<MetadataCandidates> {
    await latency();
    const user = currentUser(token);
    if (user.role === "user") fail(403, "forbidden", "Editor role required");
    const game = visibleGames().find((g) => g.id === id);
    if (!game) fail(404, "game_not_found", "That game is not in the library");
    const query = (params.q ?? game.title).trim();
    if (!query) fail(400, "invalid_query", "The search query is required");
    const all = demoCandidates(game);
    const candidates = all
      .filter((candidate) => !params.provider || candidate.provider === params.provider)
      .sort((a, b) => b.score - a.score);
    const providers = [...new Set(all.map((candidate) => candidate.provider))].map((slug) => ({
      slug,
      name: all.find((candidate) => candidate.provider === slug)?.providerName ?? slug,
      ok: true,
      error: null,
      candidates: all.filter((candidate) => candidate.provider === slug).length,
      elapsedMs: 42,
    }));
    return {
      query,
      gameId: id,
      manualFields: [...(manualFields.get(id) ?? [])],
      candidates,
      providers,
    };
  },

  async applyMetadata(token: string | null, id: number, body: MetadataApplyInput): Promise<GameDetail> {
    await latency();
    const user = currentUser(token);
    if (user.role === "user") fail(403, "forbidden", "Editor role required");
    const game = games.find((g) => g.id === id);
    if (!game) fail(404, "game_not_found", "That game is not in the library");
    const candidate = demoCandidates(game).find(
      (entry) => entry.provider === body.provider && entry.providerId === body.providerId,
    );
    if (!candidate) fail(404, "metadata_candidate_not_found", "The provider no longer returns this record");
    const marked = manualFields.get(id);
    const keep = (field: string) => !body.replaceManual && (marked?.has(field) ?? false);
    if (!keep("title")) {
      game.title = candidate.title;
      game.sortTitle = candidate.title.toLowerCase();
    }
    if (!keep("releaseDate") && candidate.year) game.releaseDate = `${candidate.year}-01-01T00:00:00Z`;
    if (!keep("description")) game.description = candidate.summary;
    if (!keep("developers")) game.developers = candidate.developers;
    game.coverUrl = candidate.coverUrl ?? game.coverUrl;
    game.provider = { slug: candidate.provider, id: candidate.providerId, url: candidate.providerUrl };
    if (body.replaceManual) manualFields.delete(id);
    return clone(game);
  },

  async setProgress(token: string | null, id: number, state: ProgressState) {
    await latency();
    const user = currentUser(token);
    const game = games.find((g) => g.id === id);
    if (!game) fail(404, "game_not_found", "That game is not in the library");
    const existed = game.state !== "unplayed" || game.minutesPlayed > 0 || Boolean(game.lastPlayedAt);
    game.state = state;
    if (state === "playing" && !game.lastPlayedAt) game.lastPlayedAt = nowIso();
    if (!existed) recordActivity(user, "game_added", { gameId: id });
    if (state === "playing") recordActivity(user, "game_played", { gameId: id, dedupeMinutes: 30 });
    if (state === "completed") recordActivity(user, "game_completed", { gameId: id, dedupeMinutes: 60 });
    return { state: game.state, minutesPlayed: game.minutesPlayed, lastPlayedAt: game.lastPlayedAt };
  },

  async addPlaytime(token: string | null, id: number, minutes: number) {
    await latency();
    const user = currentUser(token);
    const game = games.find((g) => g.id === id);
    if (!game) fail(404, "game_not_found", "That game is not in the library");
    const existed = game.state !== "unplayed" || game.minutesPlayed > 0 || Boolean(game.lastPlayedAt);
    game.minutesPlayed += Math.max(0, Math.round(minutes));
    game.lastPlayedAt = nowIso();
    if (game.state === "unplayed" || game.state === "planToPlay") game.state = "playing";
    if (!existed) recordActivity(user, "game_added", { gameId: id });
    recordActivity(user, "game_played", { gameId: id, metadata: { minutes }, dedupeMinutes: 30 });
    return { state: game.state, minutesPlayed: game.minutesPlayed, lastPlayedAt: game.lastPlayedAt };
  },

  async setBookmark(token: string | null, id: number, bookmarked: boolean) {
    await latency();
    currentUser(token);
    const game = games.find((g) => g.id === id);
    if (!game) fail(404, "game_not_found", "That game is not in the library");
    game.bookmarked = bookmarked;
    return { bookmarked };
  },

  async stats(token: string | null): Promise<LibraryStats> {
    await latency();
    currentUser(token);
    const list = visibleGames();
    const byState: Record<ProgressState, number> = {
      unplayed: 0,
      playing: 0,
      completed: 0,
      abandoned: 0,
      onHold: 0,
      planToPlay: 0,
    };
    for (const g of list) byState[g.state] += 1;
    return {
      games: list.length,
      versions: list.reduce((n, g) => n + g.versions.length, 0),
      totalBytes: list.reduce((n, g) => n + g.totalSize, 0),
      users: users.length,
      byState,
    };
  },

  async listUsers(token: string | null): Promise<User[]> {
    await latency();
    const user = currentUser(token);
    if (user.role !== "admin") fail(403, "forbidden", "Admin role required");
    return clone(users);
  },

  async createUser(
    token: string | null,
    body: { username: string; password: string; email?: string | null; role: Role },
  ): Promise<User> {
    await latency();
    const me = currentUser(token);
    if (me.role !== "admin") fail(403, "forbidden", "Admin role required");
    if (users.some((u) => u.username.toLowerCase() === body.username.toLowerCase()))
      fail(409, "username_taken", "That username is already in use");
    if (body.password.length < 8) fail(400, "weak_password", "Use at least eight characters");
    const user: User = {
      id: Math.max(0, ...users.map((u) => u.id)) + 1,
      username: body.username.trim(),
      email: body.email?.trim() || null,
      role: body.role,
      activated: true,
      avatarUrl: null,
      createdAt: nowIso(),
      ageLimit: null,
      source: "local",
      oidcIssuer: null,
      onboardingCompletedAt: null,
      onboardingStep: 0,
      mfaEnabled: false,
      mustChangePassword: false,
    };
    users = [...users, user];
    persistUsers();
    return clone(user);
  },

  async updateUser(
    token: string | null,
    id: number,
    patch: {
      username?: string;
      email?: string | null;
      password?: string;
      role?: Role;
      activated?: boolean;
      ageLimit?: number | null;
    },
  ): Promise<User> {
    await latency();
    const me = currentUser(token);
    if (me.role !== "admin") fail(403, "forbidden", "Admin role required");
    const user = users.find((u) => u.id === id);
    if (!user) fail(404, "user_not_found", "That user does not exist");

    const nextUsername = patch.username?.trim();
    const usernameChanged = nextUsername !== undefined && nextUsername !== user.username;
    const nextEmail = patch.email === undefined ? undefined : patch.email?.trim() || null;
    const emailChanged = nextEmail !== undefined && nextEmail !== user.email;
    if (user.source === "oidc" && (usernameChanged || emailChanged || patch.password !== undefined))
      fail(
        409,
        "managed_by_idp",
        `This account is managed by ${user.oidcIssuer ?? "your identity provider"}; edit it in the identity provider`,
      );
    if (patch.username !== undefined) {
      if (!nextUsername || nextUsername.length < 3) fail(400, "invalid_username", "Use at least three characters");
      if (users.some((u) => u.id !== id && u.username.toLowerCase() === nextUsername.toLowerCase()))
        fail(409, "username_taken", "That username is already in use");
    }
    if (patch.password !== undefined && patch.password.length < 8)
      fail(400, "invalid_password", "Use at least eight characters");
    if (patch.role && patch.role !== "admin" && user.role === "admin" && users.filter((u) => u.role === "admin").length === 1)
      fail(409, "last_admin", "Keep at least one administrator");
    if (patch.activated === false && user.id === me.id) fail(409, "self_deactivate", "You cannot deactivate yourself");

    if (nextUsername !== undefined) user.username = nextUsername;
    if (nextEmail !== undefined) user.email = nextEmail;
    if (patch.password !== undefined) passwords.set(user.id, patch.password);
    if (patch.role) user.role = patch.role;
    if (patch.activated !== undefined) user.activated = patch.activated;
    if (patch.ageLimit !== undefined && patch.ageLimit !== null) {
      if (!Number.isInteger(patch.ageLimit) || patch.ageLimit < 0 || patch.ageLimit > 21)
        fail(400, "invalid_age_limit", "The age limit must be between 0 and 21");
    }
    if (patch.ageLimit !== undefined) user.ageLimit = patch.ageLimit;
    persistUsers();
    return clone(user);
  },

  async deleteUser(token: string | null, id: number): Promise<void> {
    await latency();
    const me = currentUser(token);
    if (me.role !== "admin") fail(403, "forbidden", "Admin role required");
    if (id === me.id) fail(409, "self_delete", "You cannot delete your own account");
    const target = users.find((u) => u.id === id);
    if (!target) fail(404, "user_not_found", "That user does not exist");
    if (target.role === "admin" && users.filter((u) => u.role === "admin").length === 1)
      fail(409, "last_admin", "Keep at least one administrator");
    users = users.filter((u) => u.id !== id);
    persistUsers();
  },

  async scanStatus(token: string | null): Promise<ScanStatus> {
    await latency();
    currentUser(token);
    return { ...scan, errors: [...scan.errors] };
  },

  async startScan(token: string | null): Promise<{ started: boolean }> {
    await latency();
    const user = currentUser(token);
    if (user.role === "user") fail(403, "forbidden", "Editor role required");
    if (!scan.running) {
      scan = {
        running: true,
        startedAt: nowIso(),
        finishedAt: null,
        scanned: 0,
        added: 0,
        updated: 0,
        removed: 0,
        metadataPending: 0,
        errors: [],
      };
    }
    return { started: true };
  },

  async folders(token: string | null) {
    await latency();
    const user = currentUser(token);
    if (user.role !== "admin") fail(403, "forbidden", "Admin role required");
    return { gamesDir: "/srv/ludex/games", mediaDir: "/srv/ludex/media", dataDir: "/srv/ludex/data" };
  },

  async listJobs(token: string | null): Promise<IngestJob[]> {
    await latency();
    const user = currentUser(token);
    if (user.role === "user") fail(403, "forbidden", "Editor role required");
    return clone(jobs).sort((a, b) => Date.parse(b.createdAt) - Date.parse(a.createdAt));
  },

  async createJob(token: string | null, body: { url: string; title?: string }): Promise<IngestJob> {
    await latency();
    const user = currentUser(token);
    if (user.role === "user") fail(403, "forbidden", "Editor role required");
    if (!/^https?:\/\//i.test(body.url)) fail(400, "invalid_url", "Use an http or https URL");
    const fileName = decodeURIComponent(body.url.split("/").pop() ?? "archive.zip").replace(/[?&].*$/, "");
    const total = Math.round((6 + Math.random() * 40) * GB);
    const job: IngestJob = {
      id: `job_${Math.random().toString(16).slice(2, 8)}`,
      title: body.title?.trim() || fileName.replace(/\.(zip|rar|7z|exe)$/i, ""),
      url: body.url,
      status: "downloading",
      fileName,
      downloadedBytes: 0,
      totalBytes: total,
      speedBps: Math.round((38 + Math.random() * 40) * MB),
      attempt: 1,
      error: null,
      createdAt: nowIso(),
      updatedAt: nowIso(),
    };
    jobs = [job, ...jobs];
    return clone(job);
  },

  async getJob(token: string | null, id: string): Promise<IngestJob> {
    await latency();
    currentUser(token);
    const job = jobs.find((j) => j.id === id);
    if (!job) fail(404, "job_not_found", "That download no longer exists");
    return clone(job);
  },

  async cancelJob(token: string | null, id: string): Promise<void> {
    await latency();
    const user = currentUser(token);
    if (user.role === "user") fail(403, "forbidden", "Editor role required");
    const job = jobs.find((j) => j.id === id);
    if (!job) fail(404, "job_not_found", "That download no longer exists");
    if (job.status === "done") fail(409, "job_finished", "This download already finished");
    job.status = "cancelled";
    job.speedBps = 0;
    job.updatedAt = nowIso();
  },

  async retryJob(token: string | null, id: string): Promise<IngestJob> {
    await latency();
    const user = currentUser(token);
    if (user.role === "user") fail(403, "forbidden", "Editor role required");
    const job = jobs.find((j) => j.id === id);
    if (!job) fail(404, "job_not_found", "That download no longer exists");
    if (job.status === "done" || job.status === "downloading")
      fail(409, "job_running", "This download is already running");
    job.status = "downloading";
    job.attempt += 1;
    job.error = null;
    job.speedBps = Math.round((42 + Math.random() * 30) * MB);
    job.updatedAt = nowIso();
    return clone(job);
  },

  async replaceJobUrl(token: string | null, id: string, url: string): Promise<IngestJob> {
    await latency();
    const user = currentUser(token);
    if (user.role === "user") fail(403, "forbidden", "Editor role required");
    if (!/^https?:\/\//i.test(url)) fail(400, "invalid_url", "Use an http or https URL");
    const job = jobs.find((j) => j.id === id);
    if (!job) fail(404, "job_not_found", "That download no longer exists");
    job.url = url;
    if (job.status === "error" || job.status === "cancelled") {
      job.status = "downloading";
      job.attempt += 1;
      job.error = null;
      job.speedBps = 46 * MB;
    }
    job.updatedAt = nowIso();
    return clone(job);
  },

  async listFiles(token: string | null): Promise<LibraryFile[]> {
    await latency();
    const user = currentUser(token);
    if (user.role === "user") fail(403, "forbidden", "Editor role required");
    const files: LibraryFile[] = visibleGames()
      .flatMap((g) => g.versions)
      .map((v, i) => ({
        name: v.fileName,
        size: v.size,
        partial: false,
        modifiedAt: new Date(Date.now() - (i + 1) * 1000 * 60 * 60 * 19).toISOString(),
      }));
    const active = jobs.find((j) => j.status === "downloading");
    if (active?.fileName) {
      files.unshift({
        name: `${active.fileName}.part`,
        size: active.downloadedBytes,
        partial: true,
        modifiedAt: active.updatedAt,
      });
    }
    return files;
  },

  async deleteFile(token: string | null, name: string): Promise<void> {
    await latency();
    const user = currentUser(token);
    if (user.role === "user") fail(403, "forbidden", "Editor role required");
    if (!name) fail(400, "invalid_name", "Missing file name");
  },

  async verifyFile(token: string | null, name: string): Promise<{ ok: boolean; log: string }> {
    await sleep(700);
    const user = currentUser(token);
    if (user.role === "user") fail(403, "forbidden", "Editor role required");
    verify = {
      name,
      ok: true,
      log: `Testing ${name}\nHeader OK (zip, 1 volume)\nCentral directory OK\nCRC check OK — 0 errors\nArchive is intact.`,
    };
    return { ok: verify.ok, log: verify.log };
  },

  async ingestStats(token: string | null): Promise<IngestStats> {
    await latency();
    currentUser(token);
    const libraryBytes = visibleGames().reduce((n, g) => n + g.totalSize, 0);
    return {
      gamesDir: "/srv/ludex/games",
      diskTotal: 4 * 1024 * GB,
      diskFree: 1.62 * 1024 * GB,
      libraryBytes,
      gameCount: visibleGames().length,
      activeJobs: jobs.filter((j) => j.status === "downloading" || j.status === "queued").length,
    };
  },

  async listSaves(token: string | null, gameId: number): Promise<SaveSnapshot[]> {
    await latency();
    currentUser(token);
    return clone(
      saves
        .filter((save) => save.gameId === gameId)
        .sort((a, b) => Date.parse(b.createdAt) - Date.parse(a.createdAt)),
    );
  },

  async uploadSave(token: string | null, gameId: number, file: File, label?: string): Promise<SaveSnapshot> {
    await latency();
    currentUser(token);
    if (!file) fail(400, "missing_file", "Choose a save archive first");
    if (file.size > 2 * 1024 ** 3) fail(413, "file_too_large", "Saves are limited to 2 GiB");
    const snapshot: SaveSnapshot = {
      id: nextSaveId++,
      gameId,
      versionId: null,
      label: label?.trim() || file.name.replace(/\.zip$/i, ""),
      fileName: file.name,
      size: file.size,
      sha256: demoHash(`${file.name}:${file.size}`),
      createdAt: nowIso(),
      source: "manual",
      deviceId: null,
    };
    saves = [snapshot, ...saves];
    return clone(snapshot);
  },

  async deleteSave(token: string | null, id: number): Promise<void> {
    await latency();
    currentUser(token);
    saves = saves.filter((save) => save.id !== id);
  },

  async downloadSave(token: string | null, id: number): Promise<void> {
    await latency();
    currentUser(token);
    if (!saves.some((save) => save.id === id)) fail(404, "save_not_found", "That save no longer exists");
  },

  async listNews(): Promise<NewsPost[]> {
    await latency();
    return clone(news).sort((a, b) => Date.parse(b.createdAt) - Date.parse(a.createdAt));
  },

  async createNews(token: string | null, body: { title: string; body: string }): Promise<NewsPost> {
    await latency();
    const user = currentUser(token);
    if (user.role !== "admin") fail(403, "forbidden", "Admin role required");
    if (!body.title.trim()) fail(400, "invalid_title", "Give the post a title");
    const post: NewsPost = {
      id: Math.max(0, ...news.map((n) => n.id)) + 1,
      title: body.title.trim(),
      body: body.body,
      author: user.username,
      createdAt: nowIso(),
      updatedAt: nowIso(),
    };
    news = [post, ...news];
    return clone(post);
  },

  async updateNews(token: string | null, id: number, body: { title?: string; body?: string }): Promise<NewsPost> {
    await latency();
    const user = currentUser(token);
    if (user.role !== "admin") fail(403, "forbidden", "Admin role required");
    const post = news.find((n) => n.id === id);
    if (!post) fail(404, "post_not_found", "That post no longer exists");
    if (body.title !== undefined) post.title = body.title.trim();
    if (body.body !== undefined) post.body = body.body;
    post.updatedAt = nowIso();
    return clone(post);
  },

  async deleteNews(token: string | null, id: number): Promise<void> {
    await latency();
    const user = currentUser(token);
    if (user.role !== "admin") fail(403, "forbidden", "Admin role required");
    news = news.filter((n) => n.id !== id);
  },

  /* collections */
  async listCollections(token: string | null): Promise<Collection[]> {
    await latency();
    const user = currentUser(token);
    return clone(visibleCollections(user));
  },

  async getCollection(token: string | null, id: number): Promise<Collection> {
    await latency();
    const user = currentUser(token);
    return clone(decorateCollection(findCollection(id, user), user));
  },

  async createCollection(token: string | null, body: CollectionInput): Promise<Collection> {
    await latency();
    const user = currentUser(token);
    const name = body.name.trim();
    if (!name) fail(400, "invalid_name", "Give the collection a name");
    if (collections.some((entry) => entry.owner.id === user.id && entry.name.toLowerCase() === name.toLowerCase())) {
      fail(409, "collection_exists", "You already have a collection with that name");
    }
    const kind = body.kind ?? "manual";
    const rules = body.rules ?? { match: "all", rules: [] };
    if (kind === "smart") validateRules(rules);
    const id = nextCollectionId++;
    const now = nowIso();
    const collection: Collection = {
      id,
      name,
      description: body.description?.trim() || null,
      isPublic: Boolean(body.isPublic),
      kind,
      rules,
      gameCount: 0,
      owner: { id: user.id, username: user.username, displayName: profileFor(user).displayName },
      owned: true,
      createdAt: now,
      updatedAt: now,
    };
    collections = [...collections, collection];
    if (kind === "manual" && body.gameIds?.length) {
      const valid = body.gameIds.filter((gameId) => visibleGames().some((game) => game.id === gameId));
      collectionMembers[id] = [...new Set(valid)];
    } else {
      collectionMembers[id] = [];
    }
    recordActivity(user, "collection_created", { collectionId: id, metadata: { name, kind, gameCount: collectionMembers[id]?.length ?? 0 } });
    return clone(decorateCollection(collection, user));
  },

  async updateCollection(token: string | null, id: number, body: Partial<CollectionInput>): Promise<Collection> {
    await latency();
    const user = currentUser(token);
    const collection = ownedCollection(id, user);
    if (body.name !== undefined) {
      const name = body.name.trim();
      if (!name) fail(400, "invalid_name", "Give the collection a name");
      if (collections.some((entry) => entry.id !== id && entry.owner.id === user.id && entry.name.toLowerCase() === name.toLowerCase())) {
        fail(409, "collection_exists", "You already have a collection with that name");
      }
      collection.name = name;
    }
    if (body.description !== undefined) collection.description = body.description?.trim() || null;
    if (body.isPublic !== undefined) collection.isPublic = body.isPublic;
    if (body.rules !== undefined) {
      if (collection.kind === "smart") validateRules(body.rules);
      collection.rules = body.rules;
    }
    if (body.kind !== undefined && body.kind !== collection.kind) {
      collection.kind = body.kind;
      if (body.kind === "smart") {
        if (body.rules) validateRules(body.rules);
        else validateRules(collection.rules);
        collectionMembers[id] = [];
      }
    }
    collection.updatedAt = nowIso();
    collections = collections.map((entry) => (entry.id === id ? collection : entry));
    return clone(decorateCollection(collection, user));
  },

  async deleteCollection(token: string | null, id: number): Promise<void> {
    await latency();
    const user = currentUser(token);
    ownedCollection(id, user);
    collections = collections.filter((entry) => entry.id !== id);
    delete collectionMembers[id];
    activities = activities.map((activity) =>
      activity.collection?.id === id ? { ...activity, collection: null } : activity,
    );
  },

  async collectionGames(token: string | null, id: number, page = 1, limit = 96): Promise<GameList> {
    await latency();
    const user = currentUser(token);
    const collection = findCollection(id, user);
    const ids = collectionGameIds(collection);
    const items = visibleGames()
      .filter((game) => ids.includes(game.id))
      .sort((a, b) => b.id - a.id);
    const start = (page - 1) * limit;
    return { items: items.slice(start, start + limit).map(summary), total: items.length, page, limit };
  },

  async addCollectionGames(token: string | null, id: number, gameIds: number[]): Promise<CollectionGamesResponse> {
    await latency();
    const user = currentUser(token);
    const collection = ownedCollection(id, user);
    if (collection.kind === "smart") fail(409, "smart_collection", "Smart collections are built from rules");
    if (gameIds.some((gameId) => !visibleGames().some((game) => game.id === gameId))) {
      fail(404, "game_not_found", "One of those games is not in the library");
    }
    const current = collectionMembers[id] ?? [];
    const next = [...current];
    let added = 0;
    for (const gameId of gameIds) {
      if (!next.includes(gameId)) {
        next.push(gameId);
        added += 1;
      }
    }
    collectionMembers[id] = next;
    collection.updatedAt = nowIso();
    return { added, total: next.length };
  },

  async removeCollectionGame(token: string | null, id: number, gameId: number): Promise<void> {
    await latency();
    const user = currentUser(token);
    const collection = ownedCollection(id, user);
    if (collection.kind === "smart") fail(409, "smart_collection", "Smart collections are built from rules");
    collectionMembers[id] = (collectionMembers[id] ?? []).filter((entry) => entry !== gameId);
    collection.updatedAt = nowIso();
  },

  /* community */
  async myProfile(token: string | null): Promise<PublicProfile> {
    await latency();
    return clone(profileFor(currentUser(token)));
  },

  async updateProfile(token: string | null, patch: ProfilePatch): Promise<PublicProfile> {
    await latency();
    const user = currentUser(token);
    const profile = profileFor(user);
    if (patch.publicProfile !== undefined) profile.publicProfile = patch.publicProfile;
    if (patch.displayName !== undefined) profile.displayName = patch.displayName?.trim() || null;
    if (patch.bio !== undefined) profile.bio = patch.bio?.trim() || null;
    if (patch.displayName && patch.displayName.length > 64) fail(400, "invalid_display_name", "Display names are limited to 64 characters");
    if (patch.bio && patch.bio.length > 500) fail(400, "invalid_bio", "Bios are limited to 500 characters");
    profiles = profiles.map((entry) => (entry.id === profile.id ? profile : entry));
    return clone(profile);
  },

  async communityFeed(token: string | null, limit = 20, before?: number | null): Promise<ActivityPage> {
    await latency();
    currentUser(token);
    const publicIds = new Set(profiles.filter((profile) => profile.publicProfile).map((profile) => profile.id));
    return clone(activityPage(activities.filter((activity) => publicIds.has(activity.user.id)), limit, before));
  },

  async communityUsers(token: string | null): Promise<PublicProfile[]> {
    await latency();
    currentUser(token);
    return clone(profiles.filter((profile) => profile.publicProfile));
  },

  async communityUser(token: string | null, id: number): Promise<PublicProfileResponse> {
    await latency();
    const viewer = currentUser(token);
    const profile = profiles.find((entry) => entry.id === id);
    if (!profile || (!profile.publicProfile && viewer.id !== id)) fail(404, "profile_not_found", "Profile not found");
    const shelf = visibleGames()
      .filter((game) => game.state !== "unplayed" || game.minutesPlayed > 0 || game.bookmarked)
      .sort((a, b) => Date.parse(b.lastPlayedAt ?? "") - Date.parse(a.lastPlayedAt ?? ""));
    return clone({
      user: profile,
      stats: profileStats(id, viewer),
      library: shelf.slice(0, 60).map(summary),
      collections: collections
        .filter((collection) => collection.owner.id === id && (collection.isPublic || viewer.id === id))
        .map((collection) => decorateCollection(collection, viewer)),
    });
  },

  async communityUserActivity(token: string | null, id: number, limit = 20, before?: number | null): Promise<ActivityPage> {
    await latency();
    const viewer = currentUser(token);
    const profile = profiles.find((entry) => entry.id === id);
    if (!profile || (!profile.publicProfile && viewer.id !== id)) fail(404, "profile_not_found", "Profile not found");
    return clone(activityPage(activities.filter((activity) => activity.user.id === id), limit, before));
  },

  async reportInstalled(token: string | null, id: number): Promise<{ recorded: boolean }> {
    await latency();
    const user = currentUser(token);
    if (!visibleGames().some((game) => game.id === id)) fail(404, "game_not_found", "That game is not in the library");
    const before = activities.length;
    recordActivity(user, "game_installed", { gameId: id, metadata: { source: "desktop" }, dedupeMinutes: 60 });
    return { recorded: activities.length > before };
  },

  /* achievements */
  async gameAchievements(token: string | null, id: number): Promise<GameAchievements> {
    await latency();
    const user = currentUser(token);
    const game = visibleGames().find((entry) => entry.id === id);
    if (!game) fail(404, "game_not_found", "That game is not in the library");
    const achievements = demoAchievements(game, user.id);
    const unlocked = achievements.filter((entry) => entry.unlocked).length;
    const points = achievements.reduce((sum, entry) => sum + entry.points, 0);
    const unlockedPoints = achievements
      .filter((entry) => entry.unlocked)
      .reduce((sum, entry) => sum + entry.points, 0);
    return {
      gameId: id,
      total: achievements.length,
      unlocked,
      points,
      unlockedPoints,
      completion: achievements.length ? Math.round((unlocked / achievements.length) * 1000) / 10 : 0,
      achievements,
      lastScan: {
        scannedAt: "2026-11-08T21:10:00Z",
        providers: ["game_files"],
        found: achievements.length,
        unlocked: 0,
      },
    };
  },

  async scanAchievements(
    token: string | null,
    id: number,
    body: { installPath?: string | null } = {},
  ): Promise<AchievementScanReport> {
    await latency();
    const user = currentUser(token);
    if (!visibleGames().some((game) => game.id === id)) fail(404, "game_not_found", "That game is not in the library");
    const found = demoAchievements(visibleGames().find((game) => game.id === id)!, user.id);
    return {
      gameId: id,
      userId: user.id,
      scannedAt: nowIso(),
      found: found.length,
      unlocked: found.filter((entry) => entry.unlocked).length,
      providers: [
        {
          provider: "game_files",
          status: "ok",
          found: found.length,
          unlocked: found.filter((entry) => entry.unlocked).length,
          message: body.installPath ? null : "demo install path",
        },
      ],
      warnings: [],
    };
  },

  async meAchievements(token: string | null): Promise<UserAchievements> {
    await latency();
    const user = currentUser(token);
    return demoUserAchievements(user.id, user);
  },

  async userAchievements(token: string | null, id: number): Promise<UserAchievements> {
    await latency();
    currentUser(token);
    const profile = DEMO_PROFILES.find((entry) => entry.id === id);
    if (!profile) fail(404, "profile_not_found", "Profile not found");
    return demoUserAchievements(id, profile);
  },

  async desktopDownload(platform?: DesktopPlatform): Promise<DesktopDownload> {
    await latency();
    const base = "https://github.com/cleeryy/ludex/releases/download/v0.3.2";
    const assets = [
      { platform: "windows" as const, kind: "installer" as const, name: "Ludex_0.3.2_x64-setup.exe", url: `${base}/Ludex_0.3.2_x64-setup.exe`, sizeBytes: 18_400_000, sha256: "5c1f0f9a2ab0b0d1d4b8f7b3f0d6df5e1f7a9c0b9d8e7f6a5b4c3d2e1f0a9b8c" },
      { platform: "linux" as const, kind: "appimage" as const, name: "ludex_0.3.2_amd64.AppImage", url: `${base}/ludex_0.3.2_amd64.AppImage`, sizeBytes: 72_100_000, sha256: null },
      { platform: "linux" as const, kind: "deb" as const, name: "ludex_0.3.2_amd64.deb", url: `${base}/ludex_0.3.2_amd64.deb`, sizeBytes: 21_700_000, sha256: null },
      { platform: "macos" as const, kind: "dmg" as const, name: "Ludex_0.3.2_universal.dmg", url: `${base}/Ludex_0.3.2_universal.dmg`, sizeBytes: 24_900_000, sha256: null },
    ];
    const recommended =
      platform === undefined
        ? null
        : (assets.find((asset) => asset.platform === platform) ?? null);
    return {
      channel: "stable",
      source: "github",
      stale: false,
      version: "0.3.2",
      tag: "v0.3.2",
      releasedAt: "2026-11-10T09:30:00Z",
      releaseUrl: "https://github.com/cleeryy/ludex/releases/tag/v0.3.2",
      notes: "Cloud saves, controller profiles and a faster library scan.",
      checksumsUrl: null,
      platform: platform ?? null,
      recommended,
      assets,
    };
  },

  async config(): Promise<ServerConfig> {
    await latency();
    return {
      serverName: DEMO_SERVER_NAME,
      version: "0.1.0",
      features: { igdb: false, steam: true, ingest: true, localMode: false, oidc: demoOidc.enabled },
      oidcDisplayName: demoOidc.enabled ? demoOidc.displayName : null,
      maxUploadBytes: 32 * 1024 * GB,
    };
  },

  /* admin OIDC */
  async adminOidc(token: string | null): Promise<OidcAdminResponse> {
    await latency();
    requireDemoAdmin(token);
    return demoOidcResponse(demoOidc);
  },

  async saveOidcSettings(token: string | null, body: OidcSettingsInput): Promise<OidcAdminResponse> {
    await latency();
    requireDemoAdmin(token);
    const issuer = body.issuer === undefined ? demoOidc.issuer : body.issuer?.trim() || null;
    if (body.enabled && !issuer) fail(400, "invalid_oidc_issuer", "An issuer URL is required");
    if (issuer && !/^https?:\/\//i.test(issuer)) {
      fail(400, "invalid_oidc_issuer", "The issuer must be an absolute http(s) URL");
    }
    if (body.replaceSecret?.trim()) demoOidc.secretConfigured = true;
    if (body.clearSecret) demoOidc.secretConfigured = false;
    Object.assign(demoOidc, {
      enabled: body.enabled ?? demoOidc.enabled,
      issuer,
      clientId: body.clientId === undefined ? demoOidc.clientId : body.clientId?.trim() || null,
      redirectUrl: body.redirectUrl?.trim() || null,
      publicBaseUrl: body.publicBaseUrl?.trim() || null,
      scopes: body.scopes?.trim() || demoOidc.scopes,
      displayName: body.displayName?.trim() || "SSO",
      autoCreate: body.autoCreate ?? demoOidc.autoCreate,
      defaultRole: body.defaultRole ?? demoOidc.defaultRole,
      groupsClaim: body.groupsClaim?.trim() || "groups",
      adminGroups: (body.adminGroups ?? demoOidc.adminGroups)
        .map((group) => group.trim())
        .filter((group, index, all) => group && all.findIndex((entry) => entry.toLowerCase() === group.toLowerCase()) === index),
    });
    return demoOidcResponse(demoOidc);
  },

  async testOidcSettings(token: string | null, body: OidcSettingsInput): Promise<OidcTestResult> {
    await latency();
    requireDemoAdmin(token);
    const issuer = body.issuer?.trim() || demoOidc.issuer;
    if (!issuer || !/^https?:\/\//i.test(issuer)) {
      fail(400, "invalid_oidc_issuer", "The issuer must be an absolute http(s) URL");
    }
    return demoOidcDiscovery(issuer);
  },

  async resetOidcSettings(token: string | null): Promise<OidcAdminResponse> {
    await latency();
    requireDemoAdmin(token);
    demoOidc.enabled = false;
    demoOidc.secretConfigured = false;
    demoOidc.issuer = null;
    demoOidc.clientId = null;
    return demoOidcResponse(demoOidc);
  },

  /* admin metadata providers (#28) */
  async adminMetadataProviders(token: string | null): Promise<MetadataProvider[]> {
    await latency();
    requireDemoAdmin(token);
    return clone(demoProviders).sort((left, right) => left.priority - right.priority || left.id - right.id);
  },

  async createMetadataProvider(token: string | null, body: MetadataProviderInput): Promise<MetadataProvider> {
    await latency();
    requireDemoAdmin(token);
    const name = body.name?.trim();
    if (!name) fail(400, "invalid_provider_name", "Provider name is required");
    if (!body.baseUrl?.trim()) fail(400, "invalid_provider_url", "Base URL is required");
    if (demoProviders.some((provider) => provider.name.toLowerCase() === name.toLowerCase())) {
      fail(400, "provider_name_taken", "A provider with this name already exists");
    }
    const stamp = nowIso();
    const provider: MetadataProvider = {
      id: Math.max(0, ...demoProviders.map((entry) => entry.id)) + 1,
      name,
      baseUrl: body.baseUrl.trim(),
      secretConfigured: Boolean(body.token?.trim()),
      priority: body.priority ?? 100,
      enabled: body.enabled ?? false,
      mapping: body.mapping ?? {},
      createdAt: stamp,
      updatedAt: stamp,
    };
    demoProviders.push(provider);
    return clone(provider);
  },

  async updateMetadataProvider(token: string | null, id: number, body: MetadataProviderInput): Promise<MetadataProvider> {
    await latency();
    requireDemoAdmin(token);
    const provider = demoProviders.find((entry) => entry.id === id);
    if (!provider) fail(404, "metadata_provider_not_found", "Metadata provider not found");
    if (body.name !== undefined) {
      const name = body.name.trim();
      if (!name) fail(400, "invalid_provider_name", "Provider name is required");
      if (demoProviders.some((entry) => entry.id !== id && entry.name.toLowerCase() === name.toLowerCase())) {
        fail(400, "provider_name_taken", "A provider with this name already exists");
      }
      provider.name = name;
    }
    if (body.baseUrl !== undefined) {
      if (!body.baseUrl.trim()) fail(400, "invalid_provider_url", "Base URL is required");
      provider.baseUrl = body.baseUrl.trim();
    }
    if (body.token?.trim()) provider.secretConfigured = true;
    if (body.clearToken) provider.secretConfigured = false;
    if (body.priority !== undefined) provider.priority = body.priority;
    if (body.enabled !== undefined) provider.enabled = body.enabled;
    if (body.mapping !== undefined) provider.mapping = body.mapping;
    provider.updatedAt = nowIso();
    return clone(provider);
  },

  async deleteMetadataProvider(token: string | null, id: number): Promise<void> {
    await latency();
    requireDemoAdmin(token);
    demoProviders = demoProviders.filter((entry) => entry.id !== id);
  },

  async reorderMetadataProviders(token: string | null, ids: number[]): Promise<MetadataProvider[]> {
    await latency();
    requireDemoAdmin(token);
    ids.forEach((id, index) => {
      const provider = demoProviders.find((entry) => entry.id === id);
      if (!provider) fail(400, "invalid_provider_order", "Unknown provider id in the requested order");
      provider.priority = (index + 1) * 10;
      provider.updatedAt = nowIso();
    });
    return clone(demoProviders).sort((left, right) => left.priority - right.priority || left.id - right.id);
  },

  async testMetadataProvider(
    token: string | null,
    id: number,
    body: { title: string; year?: number },
  ): Promise<ProviderTestResult> {
    await latency();
    requireDemoAdmin(token);
    const provider = demoProviders.find((entry) => entry.id === id);
    if (!provider) fail(404, "metadata_provider_not_found", "Metadata provider not found");
    const title = body.title.trim();
    if (!title) return { ok: false, candidates: [], error: "A title is required to test a provider", elapsedMs: 0 };
    if (!provider.enabled) {
      return { ok: false, candidates: [], error: "The demo provider is disabled; enable it to test it.", elapsedMs: 42 };
    }
    return { ok: true, candidates: demoProviderCandidates(title), statusCode: 200, elapsedMs: 128 };
  },

  async health() {
    await latency();
    return { status: "ok", version: "0.1.0", uptimeSeconds: 412_800 };
  },

  /* collections */
  async installed(): Promise<InstalledGame[]> {
    return [];
  },


  /* sessions */
  async listSessions(token: string | null): Promise<AuthSession[]> {
    await latency();
    const user = currentUser(token);
    const now = new Date().toISOString();
    return [
      {
        id: 1,
        userAgent: navigator.userAgent,
        ip: "127.0.0.1",
        createdAt: now,
        lastSeenAt: now,
        expiresAt: new Date(Date.now() + 7 * 24 * 3600_000).toISOString(),
        revokedAt: null,
        device: "This browser",
        current: true,
      },
      ...(user.role === "admin"
        ? []
        : []),
    ];
  },

  async revokeSession(token: string | null, id: number): Promise<void> {
    await latency();
    currentUser(token);
    if (id === 1) fail(400, "session_current", "Sign out to revoke the current session");
  },

  async revokeOtherSessions(token: string | null): Promise<{ revoked: number }> {
    await latency();
    currentUser(token);
    return { revoked: 0 };
  },

  /* second factor (#166) */
  async mfaStatus(token: string | null): Promise<MfaStatus> {
    await latency();
    currentUser(token);
    return {
      enabled: false,
      recoveryCodesRemaining: 0,
      enabledAt: null,
      methods: [],
      pending: false,
    };
  },

  async startTotpEnrollment(token: string | null): Promise<TotpEnrollment> {
    await latency();
    const user = currentUser(token);
    const secret = "JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP";
    return {
      secret,
      otpauthUri: `otpauth://totp/Ludex:${user.username}?secret=${secret}&issuer=Ludex&algorithm=SHA1&digits=6&period=30`,
      issuer: "Ludex",
      account: user.username,
      digits: 6,
      period: 30,
    };
  },

  async activateTotp(token: string | null, code: string): Promise<TotpActivated> {
    await latency();
    currentUser(token);
    if (!/^\d{6}$/.test(code.trim())) fail(401, "invalid_mfa_code", "That code is not valid");
    return {
      mfa: {
        enabled: true,
        recoveryCodesRemaining: 10,
        enabledAt: new Date().toISOString(),
        methods: ["totp", "recovery_codes"],
        pending: false,
      },
      recoveryCodes: Array.from({ length: 10 }, (_, index) => `DEMO${index}-CODE${index}`),
    };
  },

  async disableMfa(token: string | null): Promise<MfaStatus> {
    await latency();
    currentUser(token);
    return {
      enabled: false,
      recoveryCodesRemaining: 0,
      enabledAt: null,
      methods: [],
      pending: false,
    };
  },

  async regenerateRecoveryCodes(token: string | null): Promise<RecoveryCodes> {
    await latency();
    currentUser(token);
    return { codes: Array.from({ length: 10 }, (_, index) => `DEMO${index}-CODE${index}`) };
  },

  /* invitations (#166) */
  async listInvitations(token: string | null): Promise<Invitation[]> {
    await latency();
    const user = currentUser(token);
    if (user.role !== "admin") fail(403, "forbidden", "Administrator role required");
    return [];
  },

  async createInvitation(
    token: string | null,
    body: { role?: Role; email?: string | null; note?: string | null; expiresInDays?: number; maxUses?: number },
  ): Promise<CreatedInvitation> {
    await latency();
    const user = currentUser(token);
    if (user.role !== "admin") fail(403, "forbidden", "Administrator role required");
    const now = new Date();
    const invitation: Invitation = {
      id: Date.now(),
      role: body.role ?? "user",
      email: body.email ?? null,
      note: body.note ?? null,
      createdBy: user.id,
      createdByUsername: user.username,
      createdAt: now.toISOString(),
      expiresAt: new Date(now.getTime() + (body.expiresInDays ?? 7) * 86_400_000).toISOString(),
      maxUses: body.maxUses ?? 1,
      uses: 0,
      lastUsedAt: null,
      revokedAt: null,
      status: "pending",
      path: "/register",
    };
    const inviteToken = `demo-invite-${Date.now()}`;
    return { invitation, token: inviteToken, path: `/register?invite=${inviteToken}` };
  },

  async revokeInvitation(token: string | null): Promise<void> {
    await latency();
    const user = currentUser(token);
    if (user.role !== "admin") fail(403, "forbidden", "Administrator role required");
  },

  async previewInvitation(inviteToken: string): Promise<InvitationPreview> {
    await latency();
    if (!inviteToken) fail(404, "invitation_not_found", "This invitation link is not valid");
    return {
      role: "user",
      email: null,
      note: null,
      expiresAt: new Date(Date.now() + 7 * 86_400_000).toISOString(),
      requiresPassword: true,
    };
  },

  async acceptInvitation(
    inviteToken: string,
    body: { username: string; password: string },
  ): Promise<AuthResult> {
    await latency();
    if (!inviteToken) fail(404, "invitation_not_found", "This invitation link is not valid");
    const user = users[0];
    if (!user) fail(409, "setup_required", "This server has no accounts yet");
    return { token: `mock.${user.id}.${Date.now()}`, user: { ...user, username: body.username } };
  },

  async resetUserPassword(token: string | null): Promise<ResetPasswordResult> {
    await latency();
    const user = currentUser(token);
    if (user.role !== "admin") fail(403, "forbidden", "Administrator role required");
    return {
      username: "demo",
      password: "GeneratedDemoPassword123",
      revoked: 1,
      mustChangePassword: true,
    };
  },

  /* resumable uploads */
  async initUpload(
    token: string | null,
    body: { fileName: string; size: number; sha256?: string | null },
  ): Promise<UploadSession> {
    await latency();
    const user = currentUser(token);
    if (user.role === "user") fail(403, "forbidden", "Editor role required");
    const chunkSize = 8 * 1024 * 1024;
    const now = new Date().toISOString();
    const upload: UploadSession = {
      id: `demo-upload-${Date.now().toString(36)}`,
      fileName: body.fileName,
      size: body.size,
      sha256: body.sha256 ?? null,
      chunkSize,
      totalParts: Math.max(1, Math.ceil(body.size / chunkSize)),
      receivedBytes: 0,
      receivedParts: [],
      status: "open",
      createdAt: now,
      updatedAt: now,
      gameId: null,
      versionId: null,
      error: null,
    };
    demoUploads.set(upload.id, upload);
    return clone(upload);
  },

  async getUpload(token: string | null, id: string): Promise<UploadSession> {
    await latency();
    currentUser(token);
    const upload = demoUploads.get(id);
    if (!upload) fail(404, "upload_not_found", "Upload not found");
    return clone(upload);
  },

  async uploadPart(
    token: string | null,
    id: string,
    part: number,
    chunk: Blob,
    onProgress?: (loaded: number) => void,
  ): Promise<UploadPartResponse> {
    currentUser(token);
    const upload = demoUploads.get(id);
    if (!upload) fail(404, "upload_not_found", "Upload not found");
    await sleep(220);
    onProgress?.(chunk.size);
    if (!upload.receivedParts.includes(part)) {
      upload.receivedParts = [...upload.receivedParts, part].sort((a, b) => a - b);
      upload.receivedBytes += chunk.size;
    }
    upload.updatedAt = new Date().toISOString();
    return {
      id,
      part,
      receivedBytes: upload.receivedBytes,
      receivedParts: upload.receivedParts.length,
      totalParts: upload.totalParts,
    };
  },

  async completeUpload(token: string | null, id: string): Promise<CompleteUploadResponse> {
    await latency();
    currentUser(token);
    const upload = demoUploads.get(id);
    if (!upload) fail(404, "upload_not_found", "Upload not found");
    if (upload.receivedParts.length !== upload.totalParts) {
      fail(400, "upload_incomplete", "Some chunks are still missing");
    }
    const game = visibleGames()[0];
    upload.status = "complete";
    upload.gameId = game?.id ?? 1;
    upload.versionId = game?.versions[0]?.id ?? 1;
    upload.updatedAt = new Date().toISOString();
    return {
      id,
      gameId: upload.gameId,
      versionId: upload.versionId,
      fileName: upload.fileName,
      size: upload.size,
      sha256: upload.sha256,
    };
  },

  async cancelUpload(token: string | null, id: string): Promise<void> {
    await latency();
    currentUser(token);
    demoUploads.delete(id);
  },

};

export const MOCK_PASSWORD = DEMO_PASSWORD;
