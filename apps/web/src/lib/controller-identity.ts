/* --------------------------------------------------------------------------
   Controller identity (#230).

   The Gamepad API gives us three things that survive a reconnect: the `id`
   string, the `mapping` name (inside the id on Chromium) and, sometimes, the
   USB vendor/product pair appended to the id. This module turns that into a
   fingerprint a profile can be keyed on. `gamepad.index` is connection order,
   never identity, so it is never used here.

   Chromium:  "Xbox Wireless Controller (STANDARD GAMEPAD Vendor: 045e Product: 0b13)"
   Firefox:   "xbox wireless controller" (no vendor/product block)
   Safari:    "Xbox Wireless Controller" (same limitation)

   When a physical vendor/product pair is present the fingerprint is that pair,
   so the same model in two USB ports, after a reboot or in another window is
   one profile. Without it we hash the normalised name: the product block, the
   mapping name and the punctuation are stripped first, so a tiny browser
   difference does not split one pad into two profiles.

   Two identical pads share a fingerprint, by design: the web platform cannot
   tell them apart across sessions, so they share one profile.
   -------------------------------------------------------------------------- */

/** Glyph family. `numbers` is a display theme, no pad reports it. */
export type PadFamily = "xbox" | "dualsense" | "steamdeck" | "switch" | "generic" | "numbers";

export const PAD_FAMILIES: readonly PadFamily[] = ["xbox", "dualsense", "steamdeck", "switch", "numbers", "generic"];

/** Glyph theme a device can pick in the manager. `auto` follows the pad. */
export type GlyphTheme = "auto" | "xbox" | "playstation" | "switch" | "steamdeck" | "numbers";

export const GLYPH_THEMES: readonly GlyphTheme[] = [
  "auto",
  "xbox",
  "playstation",
  "switch",
  "steamdeck",
  "numbers",
];

export function isGlyphTheme(value: unknown): value is GlyphTheme {
  return typeof value === "string" && (GLYPH_THEMES as readonly string[]).includes(value);
}

export function isPadFamily(value: unknown): value is PadFamily {
  return typeof value === "string" && (PAD_FAMILIES as readonly string[]).includes(value);
}

/** Everything the store and the manager need to know about one pad. */
export interface PadIdentity {
  /** Stable key for the device map; never derived from `gamepad.index`. */
  fingerprint: string;
  /** The `gamepad.id` as reported, kept for display and remapping. */
  rawId: string;
  /** Lower-case 4-digit USB vendor id, when the id string carries one. */
  vendor: string | null;
  /** Lower-case 4-digit USB product id, when the id string carries one. */
  product: string | null;
  /** True when the browser reports the standard Gamepad layout. */
  standard: boolean;
  /** Steam Input / ViGEm style virtual pad, mapped as Xbox. */
  virtual: boolean;
  family: PadFamily;
}

export interface PadIdentityOptions {
  /** The live `gamepad.mapping`; wins over the marker inside the id string. */
  mapping?: string | null;
}

const FAMILY_PATTERNS: ReadonlyArray<readonly [PadFamily, RegExp]> = [
  // Steam Input/ViGEm pads are Xbox pads behind a compatibility layer, and the
  // name must win over the "steam" prefix that would otherwise match a Deck.
  ["xbox", /(steam\s+(?:virtual|input|gamepad)|vigem)/],
  ["steamdeck", /(steam\s*deck|steamdeck)/],
  ["switch", /(nintendo\s*switch|switch\s*pro|pro\s*controller|joy-?con)/],
  ["xbox", /(xbox|xinput)/],
  ["dualsense", /(dualsense|dualshock|playstation|ps[345]|wireless controller|sony)/],
];

/** Pad family guessed from an id or a profile name, `null` when unknown. */
export function familyFromPadId(id: string | null | undefined): PadFamily | null {
  if (!id) return null;
  const text = id.toLowerCase();
  for (const [family, pattern] of FAMILY_PATTERNS) {
    if (pattern.test(text)) return family;
  }
  return null;
}

/** Product names, not interface copy: they stay as-is in every locale. */
export function padFamilyLabel(family: PadFamily): string {
  switch (family) {
    case "xbox":
      return "Xbox";
    case "dualsense":
      return "DualSense";
    case "steamdeck":
      return "Steam Deck";
    case "switch":
      return "Switch";
    case "numbers":
      return "Numbers";
    case "generic":
      return "Generic";
  }
}

/** Human label for a connected pad; a virtual pad says so. */
export function identityLabel(identity: PadIdentity): string {
  if (identity.virtual && /steam/i.test(identity.rawId)) return "Steam Input (virtual)";
  if (identity.virtual) return `${padFamilyLabel(identity.family)} (virtual)`;
  return padFamilyLabel(identity.family);
}

/** `usb:045e:0b13` → `045e:0b13`; `name:1a2b3c4d` → `1a2b3c4d`. */
export function shortFingerprint(fingerprint: string): string {
  return fingerprint.replace(/^(usb|name):/, "");
}

/**
 * Normalises a pad id for hashing: drop the `(STANDARD GAMEPAD Vendor: …)`
 * block, any standalone vendor/product pair, and every non-alphanumeric run.
 */
export function normalizePadName(rawId: string | null | undefined): string {
  return (rawId ?? "")
    .replace(/\((?:[^)]*(?:standard gamepad|vendor\s*:|product\s*:)[^)]*)\)/gi, " ")
    .replace(/(?:vendor|product)\s*:\s*[0-9a-f]{4}/gi, " ")
    .replace(/[^a-z0-9]+/gi, " ")
    .trim()
    .toLowerCase();
}

/** FNV-1a, 32 bits: small, stable and dependency-free. */
function hashName(value: string): string {
  let hash = 0x811c9dc5;
  for (let index = 0; index < value.length; index += 1) {
    hash ^= value.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193);
  }
  return (hash >>> 0).toString(16).padStart(8, "0");
}

function usbPart(rawId: string, kind: "vendor" | "product"): string | null {
  const match = rawId.match(new RegExp(`${kind}\\s*:\\s*([0-9a-f]{4})`, "i"));
  return match ? match[1].toLowerCase() : null;
}

/** Steam Input, ViGEm and friends: an Xbox pad manufactured by software. */
const NAMED_VIRTUAL = /(steam\s+(?:virtual|input|gamepad)|vigem|virtual\s+gamepad)/i;
const X360_NAMES = /(xbox\s*360|xinput)/i;

/**
 * The fingerprint plus the parsed metadata. `family` is the argument when the
 * id string does not name a family (a generic HID pad), otherwise the id wins.
 */
export function padFingerprint(
  id: string | null | undefined,
  family?: PadFamily,
  options: PadIdentityOptions = {},
): PadIdentity {
  const rawId = (id ?? "").trim() || "Unknown controller";
  const vendor = usbPart(rawId, "vendor");
  const product = usbPart(rawId, "product");
  const name = normalizePadName(rawId);
  const fingerprint = vendor && product ? `usb:${vendor}:${product}` : `name:${hashName(name || rawId.toLowerCase())}`;
  const virtual = NAMED_VIRTUAL.test(rawId) || (X360_NAMES.test(rawId) && !vendor);
  // A Steam Input/ViGEm pad is an Xbox pad behind a compatibility layer, so the
  // glyphs it accepts are Xbox glyphs whatever its name says.
  const detected = virtual && /steam/i.test(rawId) ? "xbox" : familyFromPadId(rawId);
  const resolvedFamily: PadFamily = detected ?? family ?? "generic";
  const standard =
    options.mapping !== undefined && options.mapping !== null
      ? options.mapping === "standard"
      : /standard\s+gamepad/i.test(rawId);
  return { fingerprint, rawId, vendor, product, standard, virtual, family: resolvedFamily };
}

/** The glyph family a theme asks for; `auto` follows the pad. */
export function glyphFamilyFor(theme: GlyphTheme, identity: PadIdentity): PadFamily {
  switch (theme) {
    case "auto":
      return identity.family;
    case "playstation":
      return "dualsense";
    case "xbox":
      return "xbox";
    case "switch":
      return "switch";
    case "steamdeck":
      return "steamdeck";
    case "numbers":
      return "numbers";
  }
}
