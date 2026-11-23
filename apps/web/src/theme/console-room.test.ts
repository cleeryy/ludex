import { describe, expect, it } from "vitest";
import {
  CONSOLE_ACCENT_SOURCES,
  CONSOLE_CORNERS,
  CONSOLE_DENSITIES,
  CONSOLE_HAPTICS,
  CONSOLE_KNOBS,
  CONSOLE_MOTIONS,
  CONSOLE_ROOM_BUNDLES,
  CONSOLE_ROOM_DESIGN,
  CONSOLE_ROOM_KEY,
  CONSOLE_ROOM_VERSION,
  CONSOLE_ROOMS,
  CONSOLE_SOUNDS,
  CONSOLE_WALLS,
  DEFAULT_CONSOLE_ROOM,
  cabinetTokens,
  consoleRoomAttributes,
  cycleChoice,
  motionProfile,
  readConsoleRoom,
  resetConsoleRoom,
  resetKnob,
  sanitizeConsoleRoom,
  soundEnabled,
  soundReduced,
  warmTokens,
  withConsoleRoom,
  writeConsoleRoom,
  type ConsoleRoomSettings,
} from "./console-room";
import { builtinTheme } from "./registry";

/* --------------------------------------------------------------------------
   Cabinet rooms (#268).

   Four rooms × eight knobs, a v3 record, the v2/v1 ladder, live application and
   reset per section. Pure functions only: the Settings preview, the screen and
   these tests resolve exactly the same values.
   -------------------------------------------------------------------------- */

function memoryStorage(entries: Record<string, string> = {}): {
  getItem: (key: string) => string | null;
  setItem: (key: string, value: string) => void;
  entries: Map<string, string>;
} {
  const map = new Map(Object.entries(entries));
  return {
    entries: map,
    getItem: (key) => map.get(key) ?? null,
    setItem: (key, value) => {
      map.set(key, value);
    },
  };
}

const storageLike = (storage: ReturnType<typeof memoryStorage>) => ({
  getItem: storage.getItem,
  setItem: storage.setItem,
  removeItem: (key: string) => {
    storage.entries.delete(key);
  },
});

describe("Cabinet rooms", () => {
  it("ships four rooms and eight knobs, no more", () => {
    expect(CONSOLE_ROOMS).toEqual(["night", "projector", "daylight", "arcade"]);
    expect(CONSOLE_KNOBS).toEqual([
      "room",
      "accent",
      "wall",
      "corners",
      "density",
      "motion",
      "sound",
      "haptics",
    ]);
    expect(CONSOLE_ACCENT_SOURCES).toEqual(["theme", "signal", "artwork"]);
    expect(CONSOLE_WALLS).toEqual(["artwork", "solid", "gradient"]);
    expect(CONSOLE_CORNERS).toEqual(["sharp", "soft", "round"]);
    expect(CONSOLE_DENSITIES).toEqual(["comfortable", "compact", "dense"]);
    expect(CONSOLE_MOTIONS).toEqual(["full", "calm", "off"]);
    expect(CONSOLE_SOUNDS).toEqual(["full", "reduced", "off"]);
    expect(CONSOLE_HAPTICS).toEqual(["off", "standard", "strong"]);
  });

  it("gives every room a full bundle and a material", () => {
    for (const room of CONSOLE_ROOMS) {
      const bundle = CONSOLE_ROOM_BUNDLES[room];
      expect(Object.keys(bundle).sort()).toEqual(
        ["corners", "density", "haptics", "motion", "sound", "wall"].sort(),
      );
      const design = CONSOLE_ROOM_DESIGN[room];
      expect(design.warm).toBeGreaterThanOrEqual(0);
      expect(design.warm).toBeLessThanOrEqual(0.12);
      expect(design.wallBlur).toBeGreaterThanOrEqual(0);
    }
    // Night is the Cabinet: art wall, soft corners, 26u of blur.
    expect(CONSOLE_ROOM_BUNDLES.night.wall).toBe("artwork");
    expect(CONSOLE_ROOM_BUNDLES.night.corners).toBe("soft");
    expect(CONSOLE_ROOM_DESIGN.night.wallBlur).toBe(26);
    // Projector spends no blur and steps the backgrounds to black.
    expect(CONSOLE_ROOM_DESIGN.projector.wallBlur).toBe(0);
    expect(CONSOLE_ROOM_DESIGN.projector.toBlack).toBeGreaterThan(0);
    // Daylight renders the app's light map.
    expect(CONSOLE_ROOM_DESIGN.daylight.polarity).toBe("light");
  });

  it("switches room as a bundle and keeps the player's accent and motion", () => {
    const start: ConsoleRoomSettings = {
      ...DEFAULT_CONSOLE_ROOM,
      accent: "artwork",
      motion: "calm",
    };
    const projector = withConsoleRoom(start, "projector");
    expect(projector.room).toBe("projector");
    expect(projector.wall).toBe(CONSOLE_ROOM_BUNDLES.projector.wall);
    expect(projector.corners).toBe("sharp");
    expect(projector.accent).toBe("artwork");
    expect(projector.motion).toBe("calm");
  });

  it("resets one knob to the room's answer and the section as a whole", () => {
    const edited: ConsoleRoomSettings = {
      ...DEFAULT_CONSOLE_ROOM,
      room: "projector",
      corners: "round",
      wall: "gradient",
    };
    expect(resetKnob(edited, "corners").corners).toBe("sharp");
    expect(resetKnob(edited, "corners").wall).toBe("gradient");
    expect(resetKnob(edited, "room")).toEqual(DEFAULT_CONSOLE_ROOM);
    const reset = resetConsoleRoom(edited);
    expect(reset.room).toBe("projector");
    expect(reset.corners).toBe("sharp");
    expect(reset.wall).toBe("solid");
  });

  it("cycles a knob and wraps, so a held direction is never a dead end", () => {
    expect(cycleChoice(CONSOLE_ROOMS, "night", -1)).toBe("arcade");
    expect(cycleChoice(CONSOLE_ROOMS, "arcade", 1)).toBe("night");
    expect(cycleChoice(CONSOLE_CORNERS, "soft", 1)).toBe("round");
  });

  it("maps the room to CSS attributes and the three engine knobs", () => {
    const attributes = consoleRoomAttributes({ ...DEFAULT_CONSOLE_ROOM, room: "daylight" });
    expect(attributes).toEqual({
      "data-console-room": "daylight",
      "data-console-wall": "artwork",
      "data-console-corners": "soft",
      "data-console-density": "comfortable",
      "data-console-motion": "full",
    });
    // Calm and Off take the reduced path; Off also lands instantly.
    expect(motionProfile(DEFAULT_CONSOLE_ROOM, false)).toEqual({ reduced: false, instant: false });
    expect(motionProfile({ ...DEFAULT_CONSOLE_ROOM, motion: "calm" }, false)).toEqual({
      reduced: true,
      instant: false,
    });
    expect(motionProfile({ ...DEFAULT_CONSOLE_ROOM, motion: "off" }, false)).toEqual({
      reduced: true,
      instant: true,
    });
    expect(motionProfile({ ...DEFAULT_CONSOLE_ROOM, motion: "full" }, true).reduced).toBe(true);
    expect(soundEnabled({ ...DEFAULT_CONSOLE_ROOM, sound: "off" })).toBe(false);
    expect(soundReduced({ ...DEFAULT_CONSOLE_ROOM, sound: "reduced" })).toBe(true);
    expect(soundEnabled(DEFAULT_CONSOLE_ROOM)).toBe(true);
  });
});

describe("Cabinet persistence", () => {
  it("writes a v3 record and reads it back", () => {
    const storage = memoryStorage();
    const settings: ConsoleRoomSettings = {
      ...DEFAULT_CONSOLE_ROOM,
      room: "arcade",
      accent: "signal",
      wall: "gradient",
    };
    expect(writeConsoleRoom(storageLike(storage), settings)).toEqual(settings);
    const raw = storage.entries.get(CONSOLE_ROOM_KEY);
    expect(raw).toBeDefined();
    expect(JSON.parse(raw ?? "{}")).toMatchObject({ v: CONSOLE_ROOM_VERSION, room: settings });
    expect(readConsoleRoom(storageLike(storage))).toEqual(settings);
  });

  it("migrates a v2 appearance record: Soft and the pastel accents are gone", () => {
    const storage = memoryStorage({
      [CONSOLE_ROOM_KEY]: JSON.stringify({
        v: 2,
        appearance: { preset: "soft", accent: "coral", background: "artwork", radius: "soft" },
      }),
    });
    const migrated = readConsoleRoom(storageLike(storage));
    expect(migrated.room).toBe("night");
    expect(migrated.corners).toBe("soft");
    expect(migrated.accent).toBe("theme");
    expect(migrated.wall).toBe("artwork");
  });

  it("migrates a Studio record to the crisp corner scale and keeps a real choice", () => {
    const storage = memoryStorage({
      [CONSOLE_ROOM_KEY]: JSON.stringify({
        v: 2,
        appearance: { preset: "studio", accent: "theme", background: "gradient", radius: "sharp" },
      }),
    });
    const migrated = readConsoleRoom(storageLike(storage));
    expect(migrated.room).toBe("night");
    expect(migrated.corners).toBe("sharp");
    expect(migrated.wall).toBe("gradient");
  });

  it("reads a bare v1 record and falls back on junk", () => {
    const bare = memoryStorage({
      [CONSOLE_ROOM_KEY]: JSON.stringify({ preset: "soft", accent: "mint", radius: "round" }),
    });
    const migrated = readConsoleRoom(storageLike(bare));
    expect(migrated.room).toBe("night");
    expect(migrated.corners).toBe("round");
    expect(migrated.accent).toBe("theme");

    const junk = memoryStorage({ [CONSOLE_ROOM_KEY]: "not json" });
    expect(readConsoleRoom(storageLike(junk))).toEqual(DEFAULT_CONSOLE_ROOM);
    expect(readConsoleRoom(null)).toEqual(DEFAULT_CONSOLE_ROOM);
  });

  it("sanitises every field alone", () => {
    expect(
      sanitizeConsoleRoom({ room: "night", accent: "coral", wall: "nope", corners: "soft" }),
    ).toEqual({
      ...DEFAULT_CONSOLE_ROOM,
      room: "night",
      accent: "theme",
      wall: "artwork",
      corners: "soft",
    });
  });
});

describe("Cabinet tokens", () => {
  const darkBase = builtinTheme("speranza").tokens;
  const lightBase = builtinTheme("bauhaus").tokens;

  it("casts the app's surfaces warm instead of shipping a palette", () => {
    const warm = warmTokens(darkBase, 0.035);
    const channel = (hex: string) => [1, 3, 5].map((offset) => Number.parseInt(hex.slice(offset, offset + 2), 16));
    const [r0, , b0] = channel(darkBase.surface);
    const [r1, , b1] = channel(warm.surface);
    // Warmer means more red than blue than the original step.
    expect(r1 - b1).toBeGreaterThan(r0 - b0);
  });

  it("keeps the dark rooms dark when the desktop is light", () => {
    const tokens = cabinetTokens({
      settings: { ...DEFAULT_CONSOLE_ROOM, room: "night" },
      tokens: lightBase,
      lightTokens: lightBase,
      darkTokens: darkBase,
      mode: "light",
    });
    // The Night room renders the app's dark map, not the light one.
    expect(tokens["bg-deep"]).not.toBe(lightBase["bg-deep"]);
    expect(Number.parseInt(tokens["bg-deep"].slice(1, 3), 16)).toBeLessThan(40);
  });

  it("renders the app's light map in the Daylight room", () => {
    const tokens = cabinetTokens({
      settings: { ...DEFAULT_CONSOLE_ROOM, room: "daylight" },
      tokens: darkBase,
      lightTokens: lightBase,
      darkTokens: darkBase,
      mode: "dark",
    });
    expect(Number.parseInt(tokens.bg.slice(1, 3), 16)).toBeGreaterThan(180);
  });

  it("validates a from-artwork accent or keeps the signal", () => {
    const accepted = cabinetTokens({
      settings: { ...DEFAULT_CONSOLE_ROOM, accent: "artwork" },
      tokens: darkBase,
      lightTokens: lightBase,
      darkTokens: darkBase,
      artworkAccent: "#3f7fbf",
      mode: "dark",
    });
    expect(accepted["signal-text"]).not.toBe(darkBase["signal-text"]);

    const rejected = cabinetTokens({
      settings: { ...DEFAULT_CONSOLE_ROOM, accent: "artwork" },
      tokens: darkBase,
      lightTokens: lightBase,
      darkTokens: darkBase,
      artworkAccent: null,
      mode: "dark",
    });
    expect(rejected.signal).toBe(darkBase.signal);
  });
});
