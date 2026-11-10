import type { Activity, Collection, GameDetail, GameVideo, GameVersion, NewsPost, ProgressState, PublicProfile, User } from "@/lib/types";
import { demoBackdrop, demoCover, demoStill } from "@/lib/art";

/**
 * Demo library. Every description, rating, date and screenshot here is
 * invented for the mock adapter; titles are used as sample data only.
 */

const GB = 1024 ** 3;

interface Seed {
  slug: string;
  title: string;
  year: string;
  rating: number;
  genres: string[];
  state: ProgressState;
  minutes: number;
  bookmarked?: boolean;
  lastPlayed?: string;
  earlyAccess?: boolean;
  age: number;
  sizeGb: number;
  version: string;
  versionCount?: number;
  description: string;
  developers: string[];
  publishers: string[];
  tags: string[];
  executable: string;
  /** Highest supported player count (defaults to 1). */
  players?: number;
  /** Provider aliases used by fuzzy search. */
  altTitles?: string[];
}

const seeds: Seed[] = [
  {
    slug: "blue-prince",
    title: "Blue Prince",
    year: "2025",
    rating: 92,
    genres: ["Puzzle", "Adventure"],
    state: "planToPlay",
    minutes: 0,
    bookmarked: true,
    age: 12,
    sizeGb: 3.4,
    version: "1.4.2",
    description:
      "A house that rearranges itself every time you cross the front door. Each room is a rule, each rule is a key, and the only thing you carry between runs is what you wrote down.",
    developers: ["Dogubomb"],
    publishers: ["Raw Fury"],
    tags: ["Single-player", "Mystery", "Notebook recommended"],
    executable: "BluePrince.exe",
  },
  {
    slug: "bodycam",
    title: "Bodycam",
    year: "2024",
    rating: 68,
    genres: ["Shooter", "Horror"],
    state: "unplayed",
    minutes: 0,
    age: 18,
    sizeGb: 22.8,
    version: "1.2",
    description:
      "A body-camera shooter where the footage is the interface. Short, loud, and built around the panic of a hallway with one exit.",
    developers: ["Reissad Studio"],
    publishers: ["Reissad Studio"],
    tags: ["Multiplayer", "Photoreal", "Short"],
    executable: "Bodycam.exe",
  },
  {
    slug: "call-of-duty-black-ops-iii",
    title: "Call of Duty: Black Ops III",
    year: "2015",
    rating: 79,
    genres: ["Shooter", "Action"],
    state: "completed",
    minutes: 1240,
    lastPlayed: "2026-09-14T20:10:00Z",
    age: 18,
    sizeGb: 98.6,
    version: "1.0.0.0",
    description:
      "A military shooter from the tail end of the 2010s, remembered mostly for its movement system and a co-op mode that refused to die.",
    developers: ["Treyarch"],
    publishers: ["Activision"],
    tags: ["Co-op", "Zombies", "Controller"],
    executable: "BlackOps3.exe",
    players: 4,
    altTitles: ["CoD: Black Ops III", "Black Ops 3"],
  },
  {
    slug: "cat-quest-ii",
    title: "Cat Quest II",
    year: "2019",
    rating: 81,
    genres: ["RPG", "Adventure"],
    state: "onHold",
    minutes: 320,
    lastPlayed: "2026-08-02T18:40:00Z",
    age: 7,
    sizeGb: 1.1,
    version: "1.4.1",
    description:
      "A small open-world RPG about a cat and a dog sharing one throne. Swords, puns, and a map that fits in your pocket.",
    developers: ["The Gentlebros"],
    publishers: ["The Gentlebros"],
    tags: ["Co-op", "Couch co-op", "Family"],
    executable: "CatQuest2.exe",
    players: 2,
  },
  {
    slug: "forza-horizon-6",
    title: "Forza Horizon 6",
    year: "2026",
    rating: 90,
    genres: ["Racing", "Simulation"],
    state: "playing",
    minutes: 480,
    lastPlayed: "2026-11-08T21:05:00Z",
    earlyAccess: true,
    age: 7,
    sizeGb: 132.4,
    version: "1.0.118",
    versionCount: 2,
    description:
      "An open-road festival set on a coastline that changes weather every ten minutes. Tune, drift, and photograph cars you will never insure.",
    developers: ["Playground Games"],
    publishers: ["Xbox Game Studios"],
    tags: ["Open world", "Wheel support", "Photo mode"],
    executable: "ForzaHorizon6.exe",
  },
  {
    slug: "hitman-3",
    title: "HITMAN 3",
    year: "2021",
    rating: 88,
    genres: ["Stealth", "Action"],
    state: "completed",
    minutes: 1860,
    lastPlayed: "2026-07-21T19:30:00Z",
    age: 18,
    sizeGb: 64.2,
    version: "3.210",
    versionCount: 2,
    description:
      "Three continents, six targets, one very patient man. A stealth sandbox where the best plan is always the one you improvise on the way out.",
    developers: ["IO Interactive"],
    publishers: ["IO Interactive"],
    tags: ["Stealth", "Replayable", "Single-player"],
    executable: "HITMAN3.exe",
  },
  {
    slug: "it-takes-two",
    title: "It Takes Two",
    year: "2021",
    rating: 89,
    genres: ["Adventure", "Platformer"],
    state: "playing",
    minutes: 260,
    bookmarked: true,
    lastPlayed: "2026-11-06T20:00:00Z",
    age: 12,
    sizeGb: 43.7,
    version: "1.0.0.2",
    description:
      "A co-op only story about two parents turned into dolls. Every chapter invents a new mechanic and then throws it away before it can get old.",
    developers: ["Hazelight Studios"],
    publishers: ["Electronic Arts"],
    tags: ["Co-op only", "Split screen", "Story rich"],
    executable: "ItTakesTwo.exe",
    players: 2,
  },
  {
    slug: "little-nightmares",
    title: "Little Nightmares",
    year: "2017",
    rating: 82,
    genres: ["Horror", "Puzzle"],
    state: "completed",
    minutes: 410,
    lastPlayed: "2026-06-11T22:15:00Z",
    age: 16,
    sizeGb: 6.8,
    version: "1.0",
    description:
      "A small hungry girl, a very large ship, and a world that treats children as furniture. Horror told without a single line of dialogue.",
    developers: ["Tarsier Studios"],
    publishers: ["Bandai Namco Entertainment"],
    tags: ["Atmospheric", "Short", "Single-player"],
    executable: "LittleNightmares.exe",
  },
  {
    slug: "little-nightmares-ii",
    title: "Little Nightmares II",
    year: "2021",
    rating: 84,
    genres: ["Horror", "Puzzle"],
    state: "completed",
    minutes: 380,
    lastPlayed: "2026-06-19T21:50:00Z",
    age: 16,
    sizeGb: 9.2,
    version: "1.0.0.1",
    description:
      "A boy, a paper bag, and a transmission tower on the horizon. The sequel trades the ship for a forest that is measurably worse.",
    developers: ["Tarsier Studios"],
    publishers: ["Bandai Namco Entertainment"],
    tags: ["Atmospheric", "Puzzle platformer", "Short"],
    executable: "LittleNightmaresII.exe",
  },
  {
    slug: "little-nightmares-iii",
    title: "Little Nightmares III",
    year: "2025",
    rating: 77,
    genres: ["Horror", "Puzzle"],
    state: "unplayed",
    minutes: 0,
    bookmarked: true,
    age: 16,
    sizeGb: 18.5,
    version: "1.0.3",
    description:
      "Two children cross a desert of broken things. The third entry widens the world and keeps the silence.",
    developers: ["Supermassive Games"],
    publishers: ["Bandai Namco Entertainment"],
    tags: ["Co-op", "Atmospheric", "New"],
    executable: "LittleNightmaresIII.exe",
  },
  {
    slug: "marvels-spider-man-2",
    title: "Marvel's Spider-Man 2",
    year: "2023",
    rating: 90,
    genres: ["Action", "Adventure"],
    state: "playing",
    minutes: 620,
    lastPlayed: "2026-11-09T18:20:00Z",
    age: 16,
    sizeGb: 86.3,
    version: "1.2.1",
    description:
      "Two spider-people, one city, and a symbiote that makes every fight worse for everyone involved. Swinging is still the point.",
    developers: ["Insomniac Games"],
    publishers: ["Sony Interactive Entertainment"],
    tags: ["Open world", "Story rich", "Controller"],
    executable: "SpiderMan2.exe",
    altTitles: ["Spider-Man 2", "Spiderman 2"],
  },
  {
    slug: "minecraft-dungeons",
    title: "Minecraft Dungeons",
    year: "2020",
    rating: 74,
    genres: ["RPG", "Action"],
    state: "abandoned",
    minutes: 150,
    lastPlayed: "2026-03-03T17:10:00Z",
    age: 7,
    sizeGb: 3.9,
    version: "1.17.0",
    description:
      "A dungeon crawler in the blockiest art style in games. Loot, levels, and a difficulty curve built for two controllers on one couch.",
    developers: ["Mojang Studios"],
    publishers: ["Xbox Game Studios"],
    tags: ["Couch co-op", "Family", "Loot"],
    executable: "Dungeons.exe",
    players: 4,
  },
  {
    slug: "minecraft-dungeons-ii",
    title: "Minecraft Dungeons II",
    year: "2026",
    rating: 78,
    genres: ["RPG", "Action"],
    state: "planToPlay",
    minutes: 0,
    earlyAccess: true,
    age: 7,
    sizeGb: 12.1,
    version: "0.9.4",
    description:
      "The sequel adds mounts, a wider map, and enemies that actually flank. Still best with two controllers and a shared screen.",
    developers: ["Mojang Studios"],
    publishers: ["Xbox Game Studios"],
    tags: ["Early access", "Couch co-op", "Loot"],
    executable: "DungeonsII.exe",
  },
  {
    slug: "outer-wilds",
    title: "Outer Wilds",
    year: "2019",
    rating: 94,
    genres: ["Adventure", "Mystery"],
    state: "completed",
    minutes: 900,
    bookmarked: true,
    lastPlayed: "2026-05-02T23:00:00Z",
    age: 12,
    sizeGb: 8.7,
    version: "1.1.15",
    description:
      "A solar system stuck in a twenty-two minute loop. Nothing carries over except what you learn, which turns out to be everything.",
    developers: ["Mobius Digital"],
    publishers: ["Annapurna Interactive"],
    tags: ["Mystery", "Space", "Knowledge-gated"],
    executable: "OuterWilds.exe",
  },
  {
    slug: "rv-there-yet",
    title: "RV There Yet",
    year: "2025",
    rating: 71,
    genres: ["Simulation", "Adventure"],
    state: "unplayed",
    minutes: 0,
    earlyAccess: true,
    age: 12,
    sizeGb: 5.2,
    version: "0.6.2",
    description:
      "A co-op road trip in a camper van held together by tape and optimism. The destination matters less than the radiator.",
    developers: ["Sundae Month"],
    publishers: ["Sundae Month"],
    tags: ["Co-op", "Early access", "Physics"],
    executable: "RVThereYet.exe",
  },
  {
    slug: "sackboy-a-big-adventure",
    title: "Sackboy: A Big Adventure",
    year: "2020",
    rating: 83,
    genres: ["Platformer", "Adventure"],
    state: "onHold",
    minutes: 210,
    lastPlayed: "2026-04-18T19:00:00Z",
    age: 7,
    sizeGb: 38.4,
    version: "1.0.6",
    description:
      "A handcrafted platformer where every level looks like it was built on a kitchen table. Best played with someone who laughs at physics.",
    developers: ["Sumo Digital"],
    publishers: ["Sony Interactive Entertainment"],
    tags: ["Co-op", "Family", "Couch co-op"],
    executable: "Sackboy.exe",
    players: 4,
  },
  {
    slug: "uncharted-legacy-of-thieves-collection",
    title: "UNCHARTED: Legacy of Thieves Collection",
    year: "2022",
    rating: 87,
    genres: ["Action", "Adventure"],
    state: "completed",
    minutes: 1020,
    lastPlayed: "2026-01-27T20:45:00Z",
    age: 16,
    sizeGb: 112.8,
    version: "1.0.1",
    description:
      "Two treasure hunts in one package: a thief's last job and a lost city. Climbing, shooting, and a great deal of very old stone.",
    developers: ["Naughty Dog"],
    publishers: ["PlayStation Publishing"],
    tags: ["Story rich", "Cinematic", "Single-player"],
    executable: "Uncharted.exe",
  },
  {
    slug: "over-the-hill",
    title: "Over the Hill",
    year: "2025",
    rating: 76,
    genres: ["Racing", "Simulation"],
    state: "planToPlay",
    minutes: 0,
    age: 7,
    sizeGb: 7.4,
    version: "1.1",
    description:
      "A quiet driving game about a road, a hill, and the weather. No timer, no score, just the next bend and whoever is in the passenger seat.",
    developers: ["Field Notes Studio"],
    publishers: ["Field Notes Studio"],
    tags: ["Co-op", "Chill", "Driving"],
    executable: "OverTheHill.exe",
  },
];

const normalizeSort = (title: string) => title.toLowerCase().replace(/^(the|a|an)\s+/, "");

function buildVersions(seed: Seed, id: number): GameVersion[] {
  const base: GameVersion = {
    id: id * 10 + 1,
    fileName: `${seed.title} (${seed.year}) v${seed.version}.zip`,
    size: Math.round(seed.sizeGb * GB),
    version: seed.version,
    releaseDate: `${seed.year}-03-14T00:00:00Z`,
    earlyAccess: Boolean(seed.earlyAccess),
    downloadUrl: `/api/games/${id}/download?versionId=${id * 10 + 1}`,
  };
  if (!seed.versionCount || seed.versionCount < 2) return [base];
  const older = seed.version.split(".").map((n) => Number(n) || 0);
  older[older.length - 1] = Math.max(0, older[older.length - 1] - 1);
  return [
    base,
    {
      ...base,
      id: id * 10 + 2,
      fileName: `${seed.title} (${seed.year}) v${older.join(".")}.rar`,
      version: older.join("."),
      size: Math.round(seed.sizeGb * GB * 0.94),
      releaseDate: `${Number(seed.year)}-01-20T00:00:00Z`,
      downloadUrl: `/api/games/${id}/download?versionId=${id * 10 + 2}`,
    },
  ];
}

const TRAILER_IDS = [
  "dQw4w9WgXcQ",
  "aqz-KE-bpKQ",
  "M7lc1UVf-VE",
  "ScMzIvxBSi4",
  "ysz5S6PUM-U",
  "jNQXAC9IVRw",
];

function demoVideos(seed: Seed, index: number): GameVideo[] {
  const videoId = TRAILER_IDS[index % TRAILER_IDS.length];
  return [
    {
      title: `${seed.title} — trailer`,
      url: `https://www.youtube.com/watch?v=${videoId}`,
      thumbnailUrl: `https://img.youtube.com/vi/${videoId}/hqdefault.jpg`,
      videoId,
      provider: "youtube",
    },
  ];
}

export const DEMO_GAMES: GameDetail[] = seeds.map((seed, i) => {
  const id = i + 1;
  const versions = buildVersions(seed, id);
  const videos = demoVideos(seed, i);
  return {
    id,
    title: seed.title,
    sortTitle: normalizeSort(seed.title),
    releaseDate: `${seed.year}-10-01T00:00:00Z`,
    coverUrl: demoCover(seed.slug),
    backgroundUrl: demoBackdrop(seed.slug),
    rating: seed.rating,
    genres: seed.genres,
    earlyAccess: Boolean(seed.earlyAccess),
    versionCount: versions.length,
    totalSize: versions.reduce((sum, v) => sum + v.size, 0),
    state: seed.state,
    minutesPlayed: seed.minutes,
    bookmarked: Boolean(seed.bookmarked),
    lastPlayedAt: seed.lastPlayed ?? null,
    description: seed.description,
    screenshots: [0, 1, 2, 3].map((n) => demoStill(seed.slug, n)),
    trailers: videos,
    videos,
    developers: seed.developers,
    publishers: seed.publishers,
    tags: seed.tags,
    ageRating: seed.age,
    provider: { slug: "demo", id: `demo-${seed.slug}`, url: null },
    altTitles: seed.altTitles ?? [],
    maxPlayers: seed.players ?? 1,
    versions,
    launch: {
      executable: seed.executable,
      parameters: null,
      installerExecutable: null,
      installerParameters: null,
      uninstallerExecutable: "unins000.exe",
      uninstallerParameters: "/SILENT",
    },
  };
});

export const DEMO_USERS: User[] = [
  {
    id: 1,
    username: "mara",
    email: "mara@ludex.local",
    role: "admin",
    activated: true,
    avatarUrl: null,
    createdAt: "2026-02-11T09:00:00Z",
    source: "local",
    oidcIssuer: null,
  },
  {
    id: 2,
    username: "jonas",
    email: "jonas@ludex.local",
    role: "editor",
    activated: true,
    avatarUrl: null,
    createdAt: "2026-03-04T18:20:00Z",
    source: "oidc",
    oidcIssuer: "https://sso.cleeryy.cloud",
  },
  {
    id: 3,
    username: "ines",
    email: "ines@ludex.local",
    role: "user",
    activated: true,
    avatarUrl: null,
    createdAt: "2026-04-22T11:45:00Z",
    source: "local",
    oidcIssuer: null,
  },
  {
    id: 4,
    username: "sam",
    email: null,
    role: "user",
    activated: false,
    avatarUrl: null,
    createdAt: "2026-06-30T14:05:00Z",
    source: "local",
    oidcIssuer: null,
  },
];

export const DEMO_NEWS: NewsPost[] = [
  {
    id: 3,
    title: "Incremental scans are in",
    body: "The scanner now compares stat signatures before touching the database, so a scan over a full library takes seconds instead of minutes. Removed files are still marked missing rather than deleted: progress and metadata survive a move.\n\nScan from Library scan, or let the scheduler do it every fifteen minutes.",
    author: "mara",
    createdAt: "2026-11-04T10:20:00Z",
    updatedAt: "2026-11-04T10:20:00Z",
  },
  {
    id: 2,
    title: "Ingest survives a dropped connection",
    body: "Downloads resume from the last byte received, and the real archive format is detected from the file header rather than the URL. If a job stalls, retry it from the job list; nothing is downloaded twice.",
    author: "mara",
    createdAt: "2026-10-19T16:00:00Z",
    updatedAt: "2026-10-21T09:10:00Z",
  },
  {
    id: 1,
    title: "Ludex 0.1.0",
    body: "First public build. One binary serves the API, the library and the web UI; the desktop app adds install, launch and playtime on top of the same interface.\n\nThere is no storefront here and there never will be. Ludex indexes the archives you already own, on hardware you already run.",
    author: "mara",
    createdAt: "2026-10-01T08:00:00Z",
    updatedAt: "2026-10-01T08:00:00Z",
  },
];

export const DEMO_COLLECTIONS: Collection[] = [
  {
    id: 1,
    name: "Friday night co-op",
    description: "Two controllers, one couch, no excuses.",
    isPublic: true,
    kind: "manual",
    rules: { match: "all", rules: [] },
    gameCount: 3,
    owner: { id: 1, username: "mara", displayName: "Mara" },
    owned: true,
    createdAt: "2026-09-18T19:00:00Z",
    updatedAt: "2026-11-09T21:14:00Z",
  },
  {
    id: 2,
    name: "Recent and well rated",
    description: "Everything from the last two years that cleared 85.",
    isPublic: false,
    kind: "smart",
    rules: {
      match: "all",
      rules: [
        { field: "year", op: "gte", value: 2024 },
        { field: "rating", op: "gte", value: 85 },
      ],
    },
    gameCount: 0,
    owner: { id: 1, username: "mara", displayName: "Mara" },
    owned: true,
    createdAt: "2026-10-02T20:30:00Z",
    updatedAt: "2026-10-02T20:30:00Z",
  },
  {
    id: 3,
    name: "Horror shelf",
    description: "Keep the lights on.",
    isPublic: true,
    kind: "manual",
    rules: { match: "all", rules: [] },
    gameCount: 4,
    owner: { id: 2, username: "jonas", displayName: "Jonas" },
    owned: false,
    createdAt: "2026-08-11T09:15:00Z",
    updatedAt: "2026-10-27T18:40:00Z",
  },
  {
    id: 4,
    name: "Backlog, honestly",
    description: null,
    isPublic: false,
    kind: "manual",
    rules: { match: "all", rules: [] },
    gameCount: 4,
    owner: { id: 1, username: "mara", displayName: "Mara" },
    owned: true,
    createdAt: "2026-07-22T17:05:00Z",
    updatedAt: "2026-09-01T12:00:00Z",
  },
];

/** Manual collection membership, keyed by collection id. */
export const DEMO_COLLECTION_GAMES: Record<number, number[]> = {
  1: [7, 4, 12, 16],
  2: [],
  3: [2, 8, 9, 10],
  4: [1, 6, 11, 14],
};

export const DEMO_PROFILES: PublicProfile[] = [
  {
    id: 1,
    username: "mara",
    displayName: "Mara",
    bio: "Runs the archive. Puzzle games and long nights.",
    avatarUrl: null,
    publicProfile: true,
    createdAt: "2026-02-11T09:00:00Z",
  },
  {
    id: 2,
    username: "jonas",
    displayName: "Jonas",
    bio: "Horror, co-op, and bad decisions in racing games.",
    avatarUrl: null,
    publicProfile: true,
    createdAt: "2026-03-04T18:20:00Z",
  },
  {
    id: 3,
    username: "ines",
    displayName: "Inès",
    bio: null,
    avatarUrl: null,
    publicProfile: false,
    createdAt: "2026-04-22T11:45:00Z",
  },
  {
    id: 4,
    username: "sam",
    displayName: null,
    bio: null,
    avatarUrl: null,
    publicProfile: false,
    createdAt: "2026-06-30T14:05:00Z",
  },
];

/** Chronological; the mock adapter sorts newest first. */
export const DEMO_ACTIVITIES: Activity[] = [
  {
    id: 1,
    kind: "collection_created",
    createdAt: "2026-10-02T20:30:00Z",
    metadata: { name: "Recent and well rated", kind: "smart" },
    user: { id: 1, username: "mara", displayName: "Mara", avatarUrl: null },
    game: null,
    collection: { id: 2, name: "Recent and well rated" },
  },
  {
    id: 2,
    kind: "game_added",
    createdAt: "2026-10-14T18:02:00Z",
    metadata: {},
    user: { id: 2, username: "jonas", displayName: "Jonas", avatarUrl: null },
    game: { id: 10, title: "Little Nightmares III", coverUrl: null, releaseYear: 2025 },
    collection: null,
  },
  {
    id: 3,
    kind: "game_played",
    createdAt: "2026-10-18T21:40:00Z",
    metadata: { minutes: 95 },
    user: { id: 1, username: "mara", displayName: "Mara", avatarUrl: null },
    game: { id: 14, title: "Outer Wilds", coverUrl: null, releaseYear: 2019 },
    collection: null,
  },
  {
    id: 4,
    kind: "game_completed",
    createdAt: "2026-10-24T22:12:00Z",
    metadata: {},
    user: { id: 2, username: "jonas", displayName: "Jonas", avatarUrl: null },
    game: { id: 2, title: "Bodycam", coverUrl: null, releaseYear: 2024 },
    collection: null,
  },
  {
    id: 5,
    kind: "game_installed",
    createdAt: "2026-10-27T19:31:00Z",
    metadata: { source: "desktop" },
    user: { id: 1, username: "mara", displayName: "Mara", avatarUrl: null },
    game: { id: 14, title: "Outer Wilds", coverUrl: null, releaseYear: 2019 },
    collection: null,
  },
  {
    id: 6,
    kind: "game_played",
    createdAt: "2026-10-31T20:05:00Z",
    metadata: { minutes: 60 },
    user: { id: 2, username: "jonas", displayName: "Jonas", avatarUrl: null },
    game: { id: 7, title: "It Takes Two", coverUrl: null, releaseYear: 2021 },
    collection: null,
  },
  {
    id: 7,
    kind: "collection_created",
    createdAt: "2026-11-02T21:10:00Z",
    metadata: { name: "Friday night co-op", kind: "manual" },
    user: { id: 1, username: "mara", displayName: "Mara", avatarUrl: null },
    game: null,
    collection: { id: 1, name: "Friday night co-op" },
  },
  {
    id: 8,
    kind: "game_added",
    createdAt: "2026-11-04T18:44:00Z",
    metadata: {},
    user: { id: 1, username: "mara", displayName: "Mara", avatarUrl: null },
    game: { id: 1, title: "Blue Prince", coverUrl: null, releaseYear: 2025 },
    collection: null,
  },
  {
    id: 9,
    kind: "game_completed",
    createdAt: "2026-11-06T23:26:00Z",
    metadata: {},
    user: { id: 1, username: "mara", displayName: "Mara", avatarUrl: null },
    game: { id: 3, title: "Call of Duty: Black Ops III", coverUrl: null, releaseYear: 2015 },
    collection: null,
  },
  {
    id: 10,
    kind: "game_played",
    createdAt: "2026-11-09T21:14:00Z",
    metadata: { minutes: 120 },
    user: { id: 1, username: "mara", displayName: "Mara", avatarUrl: null },
    game: { id: 14, title: "Outer Wilds", coverUrl: null, releaseYear: 2019 },
    collection: null,
  },
];

export const DEMO_GENRES = [
  "Action",
  "Adventure",
  "Horror",
  "Mystery",
  "Platformer",
  "Puzzle",
  "Racing",
  "RPG",
  "Shooter",
  "Simulation",
  "Stealth",
];

export const DEMO_PASSWORD = "ludex";

export const DEMO_SERVER_NAME = "Ludex Archive";
