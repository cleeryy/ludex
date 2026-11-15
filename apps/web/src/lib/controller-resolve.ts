import type { ControllerProfile, ControllerPreset } from "@/lib/types";
import {
  glyphFamilyFor,
  padFingerprint,
  type GlyphTheme,
  type PadFamily,
  type PadIdentity,
} from "@/lib/controller-identity";
import {
  ensureNavigableProfile,
  presetForFamily,
  presetProfile,
  swapFaceButtons,
} from "@/lib/controller-profiles";
import {
  fallbackProfile,
  needsFallbackProfile,
  type AxisClassification,
  type FallbackPadLike,
} from "@/lib/controller-fallback";
import type {
  ControllerDeviceEntry,
  ControllerDeviceSource,
  ControllerDevices,
} from "@/lib/controller-devices";

/* --------------------------------------------------------------------------
   Which profile drives a pad (#230).

   One pure function owns the order of precedence so the console, the library
   and the manager cannot disagree:

     1. the pad's own entry in the device map (Account / Game / Preset),
     2. the automatic chain: the Console profile choice, then the per-game
        override, then the account profile, then the family preset.

   An entry with `source: "auto"` deliberately joins the automatic chain: it
   inherits whatever the account or the console picked, which is what a pad
   should do before the player customises it.

   The result is never unmapped: a non-standard pad gets the generic fallback,
   A/B is swapped when the device asks for it, and missing confirm/back/direction
   bindings are filled from the standard layout.
   -------------------------------------------------------------------------- */

export interface PadProfileSources {
  accountProfile?: ControllerProfile | null;
  /** Per-game override for `gameId`, when the current screen has one. */
  gameProfile?: ControllerProfile | null;
  gameId?: number | null;
  /** Built-in presets as the server lists them; falls back to the local ones. */
  presets?: readonly ControllerPreset[] | null;
  /** The Console profile choice (`auto`, `account`, `game:<id>`, `preset:<id>`). */
  prefsProfileId?: string | null;
  /** The live pad, used only to synthesise the generic fallback. */
  pad?: FallbackPadLike | null;
  /** Axis samples over a few frames, for hat/trigger classification. */
  samples?: readonly (readonly number[])[] | null;
  classification?: AxisClassification | null;
}

export interface ResolvedPadProfile {
  identity: PadIdentity;
  /** Stored entry for this fingerprint, `null` when the pad is new. */
  entry: ControllerDeviceEntry | null;
  source: ControllerDeviceSource;
  /** The selected profile before the fallback and the A/B swap. */
  base: ControllerProfile;
  /** What must drive input: fallback, swap and gaps applied. */
  profile: ControllerProfile;
  /** Glyph family after the device theme; `auto` follows the pad. */
  glyphFamily: PadFamily;
  swapAB: boolean;
  /** True when the profile was synthesised for a non-standard pad. */
  fallback: boolean;
}

function presetById(presets: readonly ControllerPreset[] | null | undefined, id: string): ControllerProfile | null {
  const listed = presets?.find((preset) => preset.id === id);
  return listed?.profile ?? null;
}

/**
 * Resolves a profile id from the shared vocabulary. Anything that does not
 * apply to the current screen resolves to `null` so the caller falls through.
 */
function profileForId(id: string, sources: PadProfileSources): ControllerProfile | null {
  const value = id.trim();
  if (!value || value === "auto") return null;
  if (value === "account") return sources.accountProfile ?? null;
  const game = value.match(/^game:(\d+)$/);
  if (game) {
    const gameId = Number(game[1]);
    return gameId === (sources.gameId ?? null) ? sources.gameProfile ?? null : null;
  }
  const preset = value.match(/^preset:(.+)$/);
  if (preset) {
    return presetById(sources.presets, preset[1]) ?? presetProfile(preset[1]);
  }
  return null;
}

function sourceProfile(source: ControllerDeviceSource, sources: PadProfileSources): ControllerProfile | null {
  switch (source.kind) {
    case "account":
      return sources.accountProfile ?? null;
    case "game":
      return source.gameId === (sources.gameId ?? null) ? sources.gameProfile ?? null : null;
    case "preset":
      return presetById(sources.presets, source.id) ?? presetProfile(source.id);
    case "auto":
      return null;
  }
}

/** The profile a pad uses when its device entry does not pick one. */
function automaticProfile(sources: PadProfileSources, family: PadFamily): ControllerProfile {
  const chosen = sources.prefsProfileId ? profileForId(sources.prefsProfileId, sources) : null;
  // A game override only applies to the game that is actually open.
  const game = sources.gameId === null || sources.gameId === undefined ? null : sources.gameProfile ?? null;
  return chosen ?? game ?? sources.accountProfile ?? presetForFamily(family);
}

export function resolveProfileForPad(
  padId: string | null | undefined,
  family: PadFamily,
  store: ControllerDevices | null | undefined,
  sources: PadProfileSources = {},
): ResolvedPadProfile {
  const identity = padFingerprint(padId, family, { mapping: sources.pad?.mapping ?? null });
  const entry = store?.pads?.[identity.fingerprint] ?? null;
  const source: ControllerDeviceSource = entry?.source ?? { kind: "auto" };
  const base = sourceProfile(source, sources) ?? automaticProfile(sources, identity.family);

  const fallback = needsFallbackProfile(sources.pad);
  let profile = fallback
    ? fallbackProfile(sources.pad as FallbackPadLike, {
        base,
        samples: sources.samples,
        classification: sources.classification,
      })
    : base;

  const swapAB = entry?.swapAB ?? false;
  if (swapAB) profile = swapFaceButtons(profile);
  profile = ensureNavigableProfile(profile);

  const glyphTheme: GlyphTheme = entry?.glyphTheme ?? "auto";
  return {
    identity,
    entry,
    source,
    base,
    profile,
    glyphFamily: glyphFamilyFor(glyphTheme, identity),
    swapAB,
    fallback,
  };
}
