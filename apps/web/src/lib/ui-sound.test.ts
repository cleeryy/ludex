import { describe, expect, it } from "vitest";
import { createSoundManager, MAX_VOICES, SCHEDULE_AHEAD_S, type SoundBackend, type SoundVoice } from "./ui-sound";
import { CUE_SPECS, DEFAULT_SOUND_PREFS, cueGain, type CueId, type SoundPrefs } from "./ui-sound-specs";

/* --------------------------------------------------------------------------
   The manager's rules, with a fake audio backend: mute, volume, visibility,
   gameplay, reduced motion, repeat gaps and polyphony. No AudioContext is
   created, which is also the guarantee that the rest of the suite stays silent.
   -------------------------------------------------------------------------- */

interface FakeOptions {
  /** Decoded length in seconds; `null` models a cue that is not ready. */
  duration?: (cue: CueId) => number | null;
}

function fakeBackend(options: FakeOptions = {}) {
  let time = 0;
  const started: Array<{ cue: CueId; when: number; gain: number }> = [];
  const stopped: CueId[] = [];
  const backend: SoundBackend = {
    now: () => time,
    resume: () => undefined,
    duration: (cue) =>
      options.duration ? options.duration(cue) : CUE_SPECS[cue].durationMs / 1_000,
    start: (cue, when, gain) => {
      started.push({ cue, when, gain });
      const voice: SoundVoice = { stop: () => stopped.push(cue) };
      return voice;
    },
    preload: async () => undefined,
    stopAll: () => undefined,
  };
  return {
    backend,
    started,
    stopped,
    advance(ms: number) {
      time += ms / 1_000;
    },
    get time() {
      return time;
    },
  };
}

function setup(options: FakeOptions = {}) {
  const fake = fakeBackend(options);
  let prefs: SoundPrefs = { ...DEFAULT_SOUND_PREFS };
  let visible = true;
  const manager = createSoundManager(fake.backend, () => prefs, { visible: () => visible });
  return {
    ...fake,
    manager,
    setPrefs(next: Partial<SoundPrefs>) {
      prefs = { ...prefs, ...next };
      manager.setPrefs(prefs);
    },
    hide() {
      visible = false;
    },
  };
}

describe("ui sound manager", () => {
  it("plays a decoded cue just ahead of the audio clock at volume²", () => {
    const { manager, started } = setup();
    manager.play("confirm");
    expect(started).toHaveLength(1);
    expect(started[0].cue).toBe("confirm");
    expect(started[0].when).toBeCloseTo(SCHEDULE_AHEAD_S, 5);
    expect(started[0].gain).toBeCloseTo(cueGain(CUE_SPECS.confirm, 0.45), 6);
    expect(started[0].gain).toBeCloseTo(0.45 ** 2, 6);
  });

  it("is a silent no-op when the preference is off", () => {
    const { manager, started, setPrefs } = setup();
    setPrefs({ enabled: false });
    manager.play("move");
    expect(started).toEqual([]);
  });

  it("is silent at volume 0 and clamps out-of-range volumes", () => {
    const { manager, started, setPrefs } = setup();
    setPrefs({ volume: 0 });
    manager.play("move");
    expect(started).toEqual([]);

    setPrefs({ volume: 4 });
    manager.play("move");
    expect(started).toHaveLength(1);
    expect(started[0].gain).toBeCloseTo(1, 6);

    setPrefs({ volume: -3 });
    manager.play("confirm");
    expect(started).toHaveLength(1);
  });

  it("is silent while the page is hidden", () => {
    const { manager, started, hide } = setup();
    hide();
    manager.play("move");
    expect(started).toEqual([]);
  });

  it("is silent while a game runs and stops what is already scheduled", () => {
    const { manager, started, stopped, advance } = setup();
    manager.play("move");
    expect(manager.voiceCount()).toBe(1);
    manager.setGameRunning(true);
    expect(stopped).toEqual(["move"]);
    expect(manager.voiceCount()).toBe(0);
    advance(50);
    manager.play("confirm");
    expect(started).toHaveLength(1);
    manager.setGameRunning(false);
    manager.play("confirm");
    expect(started).toHaveLength(2);
  });

  it("drops the decorative cues under reduced motion and keeps the functional ones", () => {
    const { manager, started, advance } = setup();
    manager.setReducedMotion(true);
    for (const cue of ["overlayOpen", "overlayClose", "openGroup", "closeGroup", "tabChange"] as const) {
      manager.play(cue);
      advance(200);
    }
    expect(started).toEqual([]);
    for (const cue of ["move", "confirm", "back", "error", "launch", "power", "installComplete"] as const) {
      manager.play(cue);
      advance(600);
    }
    expect(started.map((entry) => entry.cue)).toEqual([
      "move",
      "confirm",
      "back",
      "error",
      "launch",
      "power",
      "installComplete",
    ]);
  });

  it("deduplicates move at 24 ms and never repeats a cue inside its gap", () => {
    const { manager, started, advance } = setup();
    manager.play("move");
    advance(10);
    manager.play("move");
    expect(started).toHaveLength(1);
    advance(14); // exactly 24 ms after the first start
    manager.play("move");
    expect(started).toHaveLength(2);

    manager.play("confirm");
    advance(40);
    manager.play("confirm");
    expect(started.filter((entry) => entry.cue === "confirm")).toHaveLength(1);
    advance(40); // 80 ms, the confirm gap
    manager.play("confirm");
    expect(started.filter((entry) => entry.cue === "confirm")).toHaveLength(2);
  });

  it("keeps three voices per cue and eight overall, dropping the oldest", () => {
    const { manager, started, stopped, advance } = setup({ duration: () => 10 });
    for (let index = 0; index < 4; index += 1) {
      manager.play("move");
      advance(30);
    }
    expect(started.filter((entry) => entry.cue === "move")).toHaveLength(4);
    expect(manager.voiceCount()).toBe(CUE_SPECS.move.maxVoices);
    expect(stopped).toEqual(["move"]);

    const cues = ["tabChange", "confirm", "back", "overlayOpen", "overlayClose", "openGroup", "closeGroup", "error"] as const;
    for (const cue of cues) {
      advance(200);
      manager.play(cue);
    }
    expect(manager.voiceCount()).toBeLessThanOrEqual(MAX_VOICES);
    expect(stopped.length).toBeGreaterThan(1);
  });

  it("plays nothing for a cue that has not decoded yet", () => {
    const { manager, started } = setup({ duration: (cue) => (cue === "launch" ? null : 1) });
    manager.play("launch");
    expect(started).toEqual([]);
    manager.play("confirm");
    expect(started).toHaveLength(1);
  });

  it("stopAll clears every live voice", () => {
    const { manager, stopped } = setup({ duration: () => 10 });
    manager.play("move");
    manager.play("tabChange");
    expect(manager.voiceCount()).toBe(2);
    manager.stopAll();
    expect(manager.voiceCount()).toBe(0);
    expect(stopped).toEqual(["move", "tabChange"]);
  });

  it("prunes voices that outlived their buffer before applying the cap", () => {
    const { manager, started, advance } = setup();
    manager.play("move"); // 45 ms
    advance(120);
    manager.play("move");
    expect(started).toHaveLength(2);
    expect(manager.voiceCount()).toBe(1);
  });
});
