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
  Activity,
  ActivityKind,
  ActivityPage,
  AuthResult,
  Collection,
  CollectionGamesResponse,
  CollectionInput,
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
  NewsPost,
  ProfilePatch,
  ProgressState,
  PublicProfile,
  PublicProfileResponse,
  Role,
  SaveSnapshot,
  ScanStatus,
  ServerConfig,
  SetupStatus,
  SmartRules,
  SuggestResponse,
  User,
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
let users: User[] = loadUsers();
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
    label: "Before the quantum moon",
    fileName: "outer-wilds-quantum-moon.zip",
    size: Math.round(4.2 * MB),
    sha256: demoHash("outer-wilds-quantum-moon"),
    createdAt: "2026-11-08T22:14:00Z",
  },
  {
    id: 2,
    gameId: 14,
    label: "Endgame, Ash Twin",
    fileName: "outer-wilds-ash-twin.zip",
    size: Math.round(8.9 * MB),
    sha256: demoHash("outer-wilds-ash-twin"),
    createdAt: "2026-10-27T19:02:00Z",
  },
  {
    id: 3,
    gameId: 7,
    label: "Chapter 4, before the clock",
    fileName: "it-takes-two-chapter-4.zip",
    size: Math.round(12.1 * MB),
    sha256: demoHash("it-takes-two-chapter-4"),
    createdAt: "2026-11-06T20:41:00Z",
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
      if (Array.isArray(parsed)) return parsed;
    } catch {
      /* ignore corrupt demo state */
    }
  }
  if (localStorage.getItem(STORAGE_SETUP) === "1") return [];
  return clone(DEMO_USERS);
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

/* ---------------------------------- api ---------------------------------- */

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
    };
    users = [user];
    persistUsers();
    localStorage.removeItem(STORAGE_SETUP);
    return { token: `mock.${user.id}.${Date.now()}`, user };
  },

  async login(body: { username: string; password: string }): Promise<AuthResult> {
    await latency();
    const user = users.find((u) => u.username.toLowerCase() === body.username.trim().toLowerCase());
    if (!user || body.password !== DEMO_PASSWORD) fail(401, "invalid_credentials", "Invalid username or password");
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

  async changePassword(token: string | null, body: { currentPassword: string; newPassword: string }): Promise<void> {
    await latency();
    currentUser(token);
    if (body.currentPassword !== DEMO_PASSWORD) fail(400, "invalid_password", "Current password is incorrect");
    if (body.newPassword.length < 8) fail(400, "weak_password", "Use at least eight characters");
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
    return clone(game);
  },

  async refreshMetadata(token: string | null, id: number): Promise<{ queued: true }> {
    await latency();
    const user = currentUser(token);
    if (user.role === "user") fail(403, "forbidden", "Editor role required");
    if (!games.some((g) => g.id === id)) fail(404, "game_not_found", "That game is not in the library");
    return { queued: true };
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
    };
    users = [...users, user];
    persistUsers();
    return clone(user);
  },

  async updateUser(
    token: string | null,
    id: number,
    patch: { email?: string | null; role?: Role; activated?: boolean },
  ): Promise<User> {
    await latency();
    const me = currentUser(token);
    if (me.role !== "admin") fail(403, "forbidden", "Admin role required");
    const user = users.find((u) => u.id === id);
    if (!user) fail(404, "user_not_found", "That user does not exist");
    if (patch.role && patch.role !== "admin" && user.role === "admin" && users.filter((u) => u.role === "admin").length === 1)
      fail(409, "last_admin", "Keep at least one administrator");
    if (patch.activated === false && user.id === me.id) fail(409, "self_deactivate", "You cannot deactivate yourself");
    Object.assign(user, patch);
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
      label: label?.trim() || file.name.replace(/\.zip$/i, ""),
      fileName: file.name,
      size: file.size,
      sha256: demoHash(`${file.name}:${file.size}`),
      createdAt: nowIso(),
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

  async config(): Promise<ServerConfig> {
    await latency();
    return {
      serverName: DEMO_SERVER_NAME,
      version: "0.1.0",
      features: { igdb: false, steam: true, ingest: true, localMode: false, oidc: true },
      oidcDisplayName: "Authentik",
      maxUploadBytes: 32 * 1024 * GB,
    };
  },

  async health() {
    await latency();
    return { status: "ok", version: "0.1.0", uptimeSeconds: 412_800 };
  },

  /* collections */
  async installed(): Promise<InstalledGame[]> {
    return [];
  },

};

export const MOCK_PASSWORD = DEMO_PASSWORD;
