import { t } from "@/lib/i18n";
import type { FirewallRule, StreamCheckStatus } from "@/lib/types";

/**
 * Copy for the streaming host wizard (#291).
 *
 * The backend classifies the host and returns stable ids; this module owns the
 * English/French wording so a self-test finding is never a bare identifier.
 * Every string goes through `t()` so `scripts/i18n.mjs` extracts it.
 */

export interface StreamCopy {
  title: string;
  /** What to do about it; empty when the check passed. */
  fix: string;
}

/** Detection table titles, keyed by the check id Rust returns. */
export function streamCheckCopy(id: string): string {
  switch (id) {
    case "sunshine":
      return t("Sunshine binary");
    case "service":
      return t("Sunshine service");
    case "capture":
      return t("Screen capture");
    case "encoder":
      return t("Video encoder");
    case "output":
      return t("Display output");
    default:
      return t("Host check");
  }
}

/** Self-test findings, keyed by the ids in `SELF_TEST_CODES`. */
export function selfTestCopy(code: string): StreamCopy | null {
  switch (code) {
    case "sunshine-missing":
      return {
        title: t("Sunshine is not installed"),
        fix: t("Install Sunshine from your distribution or the official package, then run the self-test again."),
      };
    case "sunshine-disabled":
      return {
        title: t("The Sunshine service is disabled or stopped"),
        fix: t("Run: systemctl enable --now sunshine. Moonlight cannot connect until the service runs."),
      };
    case "capture-unavailable":
      return {
        title: t("No capture device"),
        fix: t("/dev/dri has no card. Install the GPU driver; Sunshine cannot capture without KMS."),
      };
    case "no-output":
      return {
        title: t("No display output is connected"),
        fix: t("Plug in the monitor or enable the virtual display, then rescan."),
      };
    case "display-blanked":
      return {
        title: t("The display is blanked"),
        fix: t("Wake the screen and disable screen blanking while streaming (DPMS off or an inactive session captures nothing)."),
      };
    case "wrong-output":
      return {
        title: t("The wrong output is captured"),
        fix: t("Pick a connected output below; the wizard pins it as output_name."),
      };
    case "encoder-unavailable":
      return {
        title: t("No usable video encoder"),
        fix: t("Install the VA-API driver package (mesa-va-drivers or your vendor's) and vainfo, then rescan."),
      };
    case "mtu-low":
      return {
        title: t("The stream interface MTU is low"),
        fix: t("Ludex lowers Sunshine's packet size to match; expect lower quality on this path."),
      };
    default:
      return null;
  }
}

export function statusTone(
  status: StreamCheckStatus,
): "ok" | "danger" | "signal" | "outline" {
  switch (status) {
    case "ok":
      return "ok";
    case "broken":
    case "missing":
      return "danger";
    case "warning":
      return "signal";
    default:
      return "outline";
  }
}

export function statusLabel(status: StreamCheckStatus): string {
  switch (status) {
    case "ok":
      return t("ok");
    case "warning":
      return t("warning");
    case "missing":
      return t("missing");
    case "broken":
      return t("broken");
    default:
      return t("unknown");
  }
}

/**
 * Sunshine's `packet_size` preview, matching the backend rule (#291): 80 bytes
 * of IPv6/UDP/RTP headroom, clamped to 1024..=1420. The Rust planner is the
 * authority; this only labels the interface chips.
 */
export function packetSizeForMtu(mtu: number): number {
  if (!Number.isFinite(mtu) || mtu <= 0) return 1024;
  return Math.min(1420, Math.max(1024, Math.round(mtu) - 80));
}

/** `47984-47990` or `48010` when the rule is a single port. */export function portRange(rule: Pick<FirewallRule, "first" | "last">): string {
  return rule.first === rule.last ? String(rule.first) : `${rule.first}-${rule.last}`;
}

/** `TCP 47984-47990, 48010 · UDP 47998-48010` for the consent screen. */
export function portSummary(rules: FirewallRule[]): string {
  const groups: Array<[string, string[]]> = [
    ["TCP", []],
    ["UDP", []],
  ];
  for (const rule of rules) {
    const group = groups.find(([name]) => name.toLowerCase() === rule.protocol);
    group?.[1].push(portRange(rule));
  }
  return groups
    .filter(([, ports]) => ports.length > 0)
    .map(([name, ports]) => `${name} ${ports.join(", ")}`)
    .join(" · ");
}

/** One config edit as `key: old → new`, for the dry-run table. */
export function editSummary(edit: { key: string; value: string; previous: string | null }): string {
  if (edit.previous === null) return `${edit.key} = ${edit.value}`;
  return `${edit.key}: ${edit.previous} → ${edit.value}`;
}
