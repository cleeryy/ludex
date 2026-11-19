import type { ShaderPrepState } from "@/lib/types";

/** States the background queue still owns (#293). */
export function isShaderPrepActive(state: ShaderPrepState): boolean {
  return state === "queued" || state === "running";
}

/** True when the record has something the panel can show as a cache. */
export function shaderPrepHasCache(bytes: number | null | undefined): boolean {
  return (bytes ?? 0) > 0;
}

/**
 * Badge tone of a preparation state. Pure so the palette stays consistent
 * between the game page and the Downloads section.
 */
export function shaderPrepTone(
  state: ShaderPrepState,
): "default" | "signal" | "ok" | "danger" | "outline" {
  switch (state) {
    case "running":
      return "signal";
    case "queued":
      return "outline";
    case "done":
      return "ok";
    case "failed":
      return "danger";
    default:
      return "default";
  }
}
