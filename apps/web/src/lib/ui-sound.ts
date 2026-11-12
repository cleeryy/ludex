import {
  CUE_IDS,
  CUE_SPECS,
  DEFAULT_SOUND_PREFS,
  clampVolume,
  cueGain,
  sanitizeSoundPrefs,
  type CueId,
  type SoundPrefs,
} from "./ui-sound-specs";

/* --------------------------------------------------------------------------
   Big Picture sound manager (#198).

   A module singleton with a narrow surface: `play`, `preload`, `setPrefs`,
   `setGameRunning`, `setReducedMotion`, `stopAll`. The audio graph is behind a
   `SoundBackend` interface so the test suite exercises the rules — guards,
   dedupe, polyphony, cooldown — without an AudioContext, and so a platform
   without Web Audio degrades to silence instead of throwing.

   Guards, in order, every one a silent no-op:
     enabled → volume > 0 → no game running → page visible →
     reduced motion allows the cue → the cue is decoded → repeat gap → polyphony.

   Scheduling is `currentTime + 4 ms`, never 0: a source started exactly now can
   lose its first samples on some implementations. The per-cue gain is the
   file's trim times `volume²`, and a safety compressor sits before the
   destination so overlapping cues can never clip.
   -------------------------------------------------------------------------- */

export type { CueId, SoundPrefs };

/** How far ahead of the audio clock a cue is scheduled, in seconds. */
export const SCHEDULE_AHEAD_S = 0.004;

/** Global voice ceiling; a console that lags behind the input is worse than a silent one. */
export const MAX_VOICES = 8;

export interface SoundVoice {
  stop(): void;
}

/**
 * The audio layer. `start` returns `null` when the context is not running or the
 * cue is not decoded; the manager treats that as "play nothing".
 */
export interface SoundBackend {
  now(): number;
  resume(): void;
  /** Decoded length in seconds, or null while the cue is not ready. */
  duration(cue: CueId): number | null;
  start(cue: CueId, when: number, gain: number): SoundVoice | null;
  preload(): Promise<void>;
  stopAll(): void;
}

/** Environment hooks, injectable so the rules are testable outside a browser. */
export interface SoundEnv {
  visible(): boolean;
}

export interface SoundManager {
  play(cue: CueId): void;
  preload(): Promise<void>;
  setPrefs(next: SoundPrefs): void;
  setGameRunning(running: boolean): void;
  setReducedMotion(reduce: boolean): void;
  /** Ask the context to resume after a user gesture; never throws. */
  resume(): void;
  stopAll(): void;
  /** Live voices; diagnostics and tests. */
  voiceCount(): number;
}

interface ActiveVoice {
  cue: CueId;
  startedAt: number;
  voice: SoundVoice;
}

export function createSoundManager(
  backend: SoundBackend,
  prefs: () => SoundPrefs,
  env: SoundEnv = browserEnv(),
): SoundManager {
  let currentPrefs = sanitizeSoundPrefs(prefs());
  let gameRunning = false;
  let reducedMotion = false;
  let voices: ActiveVoice[] = [];
  const lastStart = new Map<CueId, number>();

  const stop = (entry: ActiveVoice | undefined) => {
    if (!entry) return;
    try {
      entry.voice.stop();
    } catch {
      /* a voice that already ended is not an error */
    }
  };

  /** Drops voices that have outlived their buffer, so the counts stay honest. */
  const prune = (now: number) => {
    voices = voices.filter((entry) => {
      const duration = backend.duration(entry.cue);
      if (duration === null) return false;
      if (now - entry.startedAt < duration + 0.05) return true;
      return false;
    });
  };

  const stopAll = () => {
    for (const entry of voices) stop(entry);
    voices = [];
  };

  return {
    play(cue) {
      const spec = CUE_SPECS[cue];
      if (!spec) return;
      if (!currentPrefs.enabled) return;
      const volume = clampVolume(currentPrefs.volume);
      if (volume <= 0) return;
      if (gameRunning) return;
      if (!env.visible()) return;
      if (reducedMotion && !spec.essential) return;
      if (backend.duration(cue) === null) return;

      const now = backend.now();
      const previous = lastStart.get(cue);
      // The same cue cannot start twice in a row inside its repeat gap; `move`
      // is the 24 ms case, the others use their own window.
      if (previous !== undefined && (now - previous) * 1000 < spec.minGapMs) return;
      lastStart.set(cue, now);

      prune(now);
      const same = voices.filter((entry) => entry.cue === cue);
      while (same.length >= spec.maxVoices) {
        const oldest = same.shift();
        stop(oldest);
        voices = voices.filter((entry) => entry !== oldest);
      }
      while (voices.length >= MAX_VOICES) {
        stop(voices.shift());
      }

      backend.resume();
      const voice = backend.start(cue, now + SCHEDULE_AHEAD_S, cueGain(spec, volume));
      if (!voice) return;
      voices.push({ cue, startedAt: now, voice });
    },

    async preload() {
      try {
        await backend.preload();
      } catch {
        /* a failed preload is silence, not a crash */
      }
    },

    setPrefs(next) {
      currentPrefs = sanitizeSoundPrefs(next);
    },

    setGameRunning(running) {
      gameRunning = running;
      if (running) {
        // No cue may leak into gameplay, even one already scheduled.
        stopAll();
      }
    },

    setReducedMotion(reduce) {
      reducedMotion = reduce;
    },

    resume() {
      try {
        backend.resume();
      } catch {
        /* the autoplay policy refusing a resume is expected */
      }
    },

    stopAll,

    voiceCount() {
      return voices.length;
    },
  };
}

/* ------------------------------- web audio -------------------------------- */

interface WebAudioWindow extends Window {
  webkitAudioContext?: typeof AudioContext;
}

function audioContextConstructor(): typeof AudioContext | null {
  if (typeof window === "undefined") return null;
  const ctor = window.AudioContext ?? (window as WebAudioWindow).webkitAudioContext;
  return ctor ?? null;
}

function cueUrl(file: string): string {
  const base = typeof document === "undefined" ? "/" : document.baseURI;
  return new URL(`sounds/bp/${file}`, base).toString();
}

/**
 * Web Audio backend. The context is created once, suspended, and resumed on the
 * first gesture; every failure (no Web Audio, a refused decode, a pad that
 * cannot rumble) stays a silent no-op.
 */
export function createWebAudioBackend(): SoundBackend {
  let context: AudioContext | null = null;
  let master: GainNode | null = null;
  let unavailable = false;
  let loading: Promise<void> | null = null;
  const buffers = new Map<CueId, AudioBuffer>();
  const live = new Set<AudioBufferSourceNode>();

  const ensureContext = (): AudioContext | null => {
    if (context || unavailable) return context;
    const Ctor = audioContextConstructor();
    if (!Ctor) {
      unavailable = true;
      return null;
    }
    try {
      const created = new Ctor({ latencyHint: "interactive" });
      const compressor = created.createDynamicsCompressor();
      compressor.threshold.value = -12;
      compressor.knee.value = 24;
      compressor.ratio.value = 12;
      compressor.attack.value = 0.003;
      compressor.release.value = 0.15;
      const gain = created.createGain();
      gain.gain.value = 1;
      gain.connect(compressor);
      compressor.connect(created.destination);
      context = created;
      master = gain;
    } catch {
      // Older WebKit can refuse a context created outside a gesture: keep
      // `context` null so the first gesture (resume) tries again.
      context = null;
      master = null;
    }
    return context;
  };

  const decode = async (cue: CueId): Promise<void> => {
    const ctx = ensureContext();
    if (!ctx || buffers.has(cue)) return;
    try {
      const response = await fetch(cueUrl(CUE_SPECS[cue].file));
      if (!response.ok) return;
      const bytes = await response.arrayBuffer();
      const buffer = await ctx.decodeAudioData(bytes);
      buffers.set(cue, buffer);
    } catch {
      /* one missing cue stays silent; the rest of the set still plays */
    }
  };

  /**
   * Idempotent while it succeeds. A context that could not be created before a
   * gesture leaves `loading` null, so the first gesture retries instead of
   * leaving the session silent forever.
   */
  const preload = (): Promise<void> => {
    if (loading) return loading;
    if (!ensureContext()) return Promise.resolve();
    loading = Promise.all(CUE_IDS.map((cue) => decode(cue))).then(() => undefined);
    return loading;
  };

  return {
    now() {
      return context?.currentTime ?? 0;
    },

    resume() {
      const ctx = ensureContext();
      if (!ctx) return;
      void ctx.resume().catch(() => undefined);
      // The first gesture is also the second chance for a deferred context.
      if (buffers.size === 0) void preload();
    },

    duration(cue) {
      return buffers.get(cue)?.duration ?? null;
    },

    start(cue, when, gain) {
      const ctx = context;
      const buffer = buffers.get(cue);
      if (!ctx || !buffer || !master || ctx.state !== "running") return null;
      try {
        const source = ctx.createBufferSource();
        source.buffer = buffer;
        const voiceGain = ctx.createGain();
        voiceGain.gain.value = gain;
        source.connect(voiceGain);
        voiceGain.connect(master);
        source.onended = () => live.delete(source);
        source.start(Math.max(when, ctx.currentTime));
        live.add(source);
        return {
          stop() {
            try {
              source.stop();
            } catch {
              /* already ended */
            }
          },
        };
      } catch {
        return null;
      }
    },

    preload,

    stopAll() {
      for (const source of live) {
        try {
          source.stop();
        } catch {
          /* already ended */
        }
      }
      live.clear();
    },
  };
}

function browserEnv(): SoundEnv {
  return {
    visible: () => typeof document === "undefined" || document.visibilityState === "visible",
  };
}

/* -------------------------------- singleton -------------------------------- */

let singleton: SoundManager | null = null;

function manager(): SoundManager {
  if (!singleton) {
    singleton = createSoundManager(createWebAudioBackend(), () => DEFAULT_SOUND_PREFS);
  }
  return singleton;
}

/**
 * The screen's manager. Every method is lazy: importing this module in a test
 * or on the server never constructs an AudioContext.
 */
export const uiSound: SoundManager = {
  play: (cue) => manager().play(cue),
  preload: () => manager().preload(),
  setPrefs: (next) => manager().setPrefs(next),
  setGameRunning: (running) => manager().setGameRunning(running),
  setReducedMotion: (reduce) => manager().setReducedMotion(reduce),
  resume: () => manager().resume(),
  stopAll: () => manager().stopAll(),
  voiceCount: () => manager().voiceCount(),
};
