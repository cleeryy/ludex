/**
 * Remote play wizard state (#179).
 *
 * The wizard is a small explicit state machine so retries, failures and the
 * back button cannot put the UI on a step whose data does not exist yet. The
 * network calls live in `native.ts`; this module is pure and unit-tested.
 */

export type RemotePlayStep = "detect" | "mode" | "consent" | "map" | "verify" | "test";

/** LAN plays at home, VPN is the recommended remote path, UPnP is advanced. */
export type RemotePlayMode = "vpn" | "lan" | "upnp";

export interface RemotePlayWizard {
  step: RemotePlayStep;
  mode: RemotePlayMode | null;
  error: string | null;
}

export const INITIAL_REMOTE_PLAY_WIZARD: RemotePlayWizard = {
  step: "detect",
  mode: null,
  error: null,
};

export type RemotePlayWizardEvent =
  | { type: "detected" }
  | { type: "resumed" }
  | { type: "choose"; mode: RemotePlayMode }
  | { type: "consent" }
  | { type: "mapped" }
  | { type: "verified" }
  | { type: "failed"; message: string }
  | { type: "retry" }
  | { type: "back" }
  | { type: "reset" };

export function remotePlayWizard(
  state: RemotePlayWizard,
  event: RemotePlayWizardEvent,
): RemotePlayWizard {
  switch (event.type) {
    case "reset":
      return INITIAL_REMOTE_PLAY_WIZARD;
    case "detected":
      return { step: "mode", mode: null, error: null };
    case "resumed":
      // An active session skips straight to the Moonlight instructions.
      return { ...state, step: "test", error: null };
    case "choose":
      return {
        step: event.mode === "upnp" ? "consent" : "test",
        mode: event.mode,
        error: null,
      };
    case "consent":
      return { ...state, step: "map", error: null };
    case "mapped":
      return { ...state, step: "verify", error: null };
    case "verified":
      return { ...state, step: "test", error: null };
    case "failed":
      return { ...state, error: event.message };
    case "retry":
      return { ...state, error: null };
    case "back":
      switch (state.step) {
        case "mode":
          return { ...INITIAL_REMOTE_PLAY_WIZARD };
        case "consent":
          return { ...state, step: "mode", error: null };
        case "map":
        case "verify":
          return { ...state, step: "consent", error: null };
        case "test":
          return state.mode === "upnp"
            ? { ...state, step: "verify", error: null }
            : { ...state, step: "mode", error: null };
        default:
          return state;
      }
  }
}

/** One-line summary of the exact ports on the consent screen. */
export function portSummary(plan: { tcp: number[]; udp: number[] }): string {
  const tcp = plan.tcp.length > 0 ? `TCP ${plan.tcp.join(", ")}` : null;
  const udp = plan.udp.length > 0 ? `UDP ${plan.udp.join(", ")}` : null;
  return [tcp, udp].filter(Boolean).join(" · ");
}

/**
 * The address to type in Moonlight, in priority order: the VPN address, the
 * public address when direct mode is on, then the advertised address (LAN,
 * IPv6 or VPN when the backend picked one) and finally the LAN address.
 */
export function moonlightAddress(status: {
  vpn: { address: string | null };
  advertised?: { address: string | null } | null;
  publicIp: string | null;
  lanIp: string | null;
  active: { mappings: unknown[] };
}): string | null {
  if (status.vpn.address) return status.vpn.address;
  if (status.active.mappings.length > 0 && status.publicIp) return status.publicIp;
  return status.advertised?.address ?? status.lanIp;
}

/**
 * Addresses the wizard can offer, advertised first, usable kinds only (#206).
 * A Docker bridge, a CGNAT WAN address, a loopback or an IPv6 ULA is listed in
 * the detection report but never offered for Moonlight.
 */
export function addressChoices<T extends { address: string; kind: string }>(status: {
  advertised: { address: string | null };
  addresses: T[];
}): T[] {
  const usable = status.addresses.filter(
    (entry) => entry.kind === "vpn" || entry.kind === "lan" || entry.kind === "ipv6",
  );
  const chosen = status.advertised.address;
  if (!chosen) return usable;
  return [
    ...usable.filter((entry) => entry.address === chosen),
    ...usable.filter((entry) => entry.address !== chosen),
  ];
}

/** True when a mapping's remaining lease is under half of its duration. */
export function leaseNeedsRefresh(
  session: { mappings: Array<{ expiresAt: number; leaseSecs: number }> },
  nowSeconds: number,
): boolean {
  return session.mappings.some(
    (mapping) => mapping.expiresAt - nowSeconds < Math.max(1, mapping.leaseSecs / 2),
  );
}

/** Minutes left on the mapping that expires first, rounded down. */
export function minutesUntilExpiry(
  session: { mappings: Array<{ expiresAt: number }> },
  nowSeconds: number,
): number | null {
  if (session.mappings.length === 0) return null;
  const next = Math.min(...session.mappings.map((mapping) => mapping.expiresAt));
  return Math.max(0, Math.floor((next - nowSeconds) / 60));
}

/**
 * How a failed command is surfaced. Tauri rejects with the serialized
 * `RemotePlayFailure`; a plain string is kept as a fallback.
 */
export function remotePlayFailure(error: unknown): { code: string; message: string } {
  if (error && typeof error === "object") {
    const value = error as { code?: unknown; message?: unknown };
    if (typeof value.code === "string" && typeof value.message === "string") {
      return { code: value.code, message: value.message };
    }
    if (typeof value.message === "string") {
      return { code: "unknown", message: value.message };
    }
  }
  if (typeof error === "string") return { code: "unknown", message: error };
  return { code: "unknown", message: "" };
}
