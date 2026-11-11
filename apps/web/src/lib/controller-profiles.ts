import type { ControllerAction, ControllerProfile } from "@/lib/types";

/**
 * Controller profile maths (#51), mirroring `ludex_core::gamepad`.
 *
 * A profile maps physical inputs (`button0`, `axis1-`, …) to the logical
 * actions the interface understands, and shapes analog sticks with a deadzone,
 * a response curve, a sensitivity multiplier and optional inversion. The Rust
 * validator stores exactly this document, so both sides must agree on the
 * defaults and clamp ranges. The vitest suite covers the same cases as the
 * Rust tests.
 */

export const CONTROLLER_ACTIONS: ControllerAction[] = [
  "left",
  "right",
  "up",
  "down",
  "confirm",
  "back",
  "menu",
  "downloads",
  "prevTab",
  "nextTab",
];

export const CONTROLLER_PRESETS = ["xbox", "dualsense", "steamdeck", "generic"] as const;
export type ControllerPresetId = (typeof CONTROLLER_PRESETS)[number];

const STANDARD_MAPPINGS: Record<string, ControllerAction> = {
  button0: "confirm",
  button1: "back",
  button2: "menu",
  button3: "downloads",
  button4: "prevTab",
  button5: "nextTab",
  button12: "up",
  button13: "down",
  button14: "left",
  button15: "right",
  "axis0-": "left",
  "axis0+": "right",
  "axis1-": "up",
  "axis1+": "down",
};

function base(name: string): ControllerProfile {
  return {
    name,
    deadzone: 0.18,
    sensitivity: 1,
    curve: 1,
    invertX: false,
    invertY: false,
    triggerThreshold: 0.35,
    vibration: true,
    mappings: { ...STANDARD_MAPPINGS },
  };
}

/** Built-in preset by id; unknown ids fall back to Xbox. */
export function presetProfile(id: string): ControllerProfile {
  switch (id.trim().toLowerCase()) {
    case "dualsense":
    case "playstation":
    case "ps5":
      return { ...base("DualSense"), triggerThreshold: 0.25 };
    case "steamdeck":
    case "steam-deck":
    case "deck":
      return {
        ...base("Steam Deck"),
        mappings: { ...STANDARD_MAPPINGS, button16: "menu", button17: "downloads" },
      };
    case "generic":
    case "standard":
      return { ...base("Generic"), vibration: false };
    default:
      return base("Xbox");
  }
}

/** Effective profile: per-game wins, then account, then the Xbox preset. */
export function resolveControllerProfile(
  global: ControllerProfile | null | undefined,
  game: ControllerProfile | null | undefined,
): ControllerProfile {
  return game ?? global ?? presetProfile("xbox");
}

/** Corrects one axis component. */
export function correctAxis(raw: number, profile: Pick<ControllerProfile, "deadzone" | "curve" | "sensitivity">, inverted = false): number {
  const value = Math.max(-1, Math.min(1, Number.isFinite(raw) ? raw : 0));
  const magnitude = Math.abs(value);
  if (magnitude <= profile.deadzone) return 0;
  const normalised = Math.min(1, Math.max(0, (magnitude - profile.deadzone) / (1 - profile.deadzone)));
  const shaped = Math.pow(normalised, profile.curve);
  const scaled = Math.min(1, Math.max(0, shaped * profile.sensitivity));
  const signed = value < 0 ? -scaled : scaled;
  return inverted ? -signed : signed;
}

/** Corrected stick value for one component (`horizontal` = x). */
export function applyStick(profile: ControllerProfile, x: number, y: number, horizontal: boolean): number {
  return correctAxis(horizontal ? x : y, profile, horizontal ? profile.invertX : profile.invertY);
}

/** Button action, honouring the trigger threshold on LT/RT (buttons 6/7). */
export function actionForButton(profile: ControllerProfile, index: number, value: number): ControllerAction | null {
  const threshold = index === 6 || index === 7 ? profile.triggerThreshold : 0.5;
  if (value < threshold) return null;
  const action = profile.mappings[`button${index}`];
  return isControllerAction(action) ? action : null;
}

/** Dominant stick action, if any. */
export function actionForAxis(profile: ControllerProfile, axis: number, x: number, y: number): ControllerAction | null {
  if (axis < 0 || axis > 7) return null;
  const horizontal = applyStick(profile, x, y, true);
  const vertical = applyStick(profile, x, y, false);
  const key =
    Math.abs(horizontal) >= Math.abs(vertical)
      ? horizontal <= -0.5
        ? `axis${axis}-`
        : horizontal >= 0.5
          ? `axis${axis}+`
          : null
      : vertical <= -0.5
        ? `axis${axis}-`
        : vertical >= 0.5
          ? `axis${axis}+`
          : null;
  if (!key) return null;
  const action = profile.mappings[key];
  return isControllerAction(action) ? action : null;
}

export function isControllerAction(value: unknown): value is ControllerAction {
  return typeof value === "string" && (CONTROLLER_ACTIONS as string[]).includes(value);
}

/** Validates a profile; returns a human-readable reason or `null`. */
export function validateControllerProfile(profile: ControllerProfile): string | null {
  if (!profile.name.trim()) return "A profile name is required.";
  if (profile.name.length > 64) return "The profile name is limited to 64 characters.";
  if (!(profile.deadzone >= 0 && profile.deadzone <= 0.95)) return "The deadzone must be between 0 and 0.95.";
  if (!(profile.sensitivity >= 0.1 && profile.sensitivity <= 3)) return "The sensitivity must be between 0.1 and 3.";
  if (!(profile.curve >= 0.5 && profile.curve <= 4)) return "The curve must be between 0.5 and 4.";
  if (!(profile.triggerThreshold >= 0 && profile.triggerThreshold <= 1)) return "The trigger threshold must be between 0 and 1.";
  for (const [input, action] of Object.entries(profile.mappings)) {
    if (!/^(button([0-9]|[12][0-9]|3[01])|axis[0-7][-+])$/.test(input)) return `Invalid input \`${input}\`.`;
    if (!isControllerAction(action)) return `Unknown action \`${action}\`.`;
  }
  return null;
}

/** Parses an imported JSON document and validates it. */
export function parseControllerProfile(document: string): ControllerProfile {
  const parsed = JSON.parse(document) as ControllerProfile;
  const profile = {
    ...presetProfile("xbox"),
    ...parsed,
    mappings: parsed.mappings ?? {},
  };
  const error = validateControllerProfile(profile);
  if (error) throw new Error(error);
  return profile;
}

/** Pretty JSON for the export button. */
export function serialiseControllerProfile(profile: ControllerProfile): string {
  return JSON.stringify(profile, null, 2);
}

/** Human label for a physical input, used by the remap list. */
export function inputLabel(input: string): string {
  const button = input.match(/^button(\d+)$/);
  if (button) {
    const names: Record<number, string> = {
      0: "A / Cross",
      1: "B / Circle",
      2: "X / Square",
      3: "Y / Triangle",
      4: "LB / L1",
      5: "RB / R1",
      6: "LT / L2",
      7: "RT / R2",
      8: "Back / Share",
      9: "Start / Options",
      12: "D-pad up",
      13: "D-pad down",
      14: "D-pad left",
      15: "D-pad right",
    };
    return names[Number(button[1])] ?? `Button ${button[1]}`;
  }
  const axis = input.match(/^axis(\d)([-+])$/);
  if (axis) return `Stick ${Number(axis[1]) + 1} ${axis[2] === "-" ? "−" : "+"}`;
  return input;
}
