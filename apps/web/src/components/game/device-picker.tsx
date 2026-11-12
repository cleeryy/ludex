import { useQuery } from "@tanstack/react-query";
import { Monitor } from "lucide-react";
import { api } from "@/lib/api";
import { t } from "@/lib/i18n";
import type { Device } from "@/lib/types";

/**
 * Polls the account's registered PCs (#184).
 *
 * Fifteen seconds keeps the online badge and the target picker fresh without
 * hammering the server; the registry heartbeat itself is what makes a PC
 * online, not this poll.
 */
export function useDevices(enabled = true) {
  return useQuery({
    queryKey: ["devices"],
    queryFn: () => api.listDevices(),
    enabled,
    refetchInterval: 15_000,
    staleTime: 10_000,
  });
}

function platformLabel(platform: string | null): string {
  switch ((platform ?? "").toLowerCase()) {
    case "windows":
      return "Windows";
    case "macos":
      return "macOS";
    case "linux":
      return "Linux";
    default:
      return platform || t("Unknown platform");
  }
}

export function deviceSummary(device: Device): string {
  return [platformLabel(device.platform), device.appVersion ? `v${device.appVersion}` : null]
    .filter(Boolean)
    .join(" · ");
}

const PREFERRED_DEVICE_KEY = "ludex.remote.targetDevice";

/** Last PC picked as the install target, shared by the game page and queue. */
export function preferredDeviceId(): string | null {
  if (typeof window === "undefined") return null;
  try {
    return window.localStorage.getItem(PREFERRED_DEVICE_KEY);
  } catch {
    return null;
  }
}

export function rememberPreferredDevice(id: string | null) {
  if (typeof window === "undefined") return;
  try {
    if (id) window.localStorage.setItem(PREFERRED_DEVICE_KEY, id);
    else window.localStorage.removeItem(PREFERRED_DEVICE_KEY);
  } catch {
    // A blocked storage API only loses the preference, never the action.
  }
}

/**
 * "Install on <PC>" target picker. Renders nothing until at least one PC is
 * registered, which keeps the account-wide fallback as the default.
 */
export function DevicePicker({
  devices,
  value,
  onChange,
  disabled = false,
}: {
  devices: readonly Device[];
  value: string | null;
  onChange: (value: string | null) => void;
  disabled?: boolean;
}) {
  if (devices.length === 0) return null;
  return (
    <label className="flex min-w-0 items-center gap-2 text-[12px] text-ink-3">
      <Monitor className="size-3.5 shrink-0 text-signal" aria-hidden="true" />
      <span className="shrink-0">{t("Install on")}</span>
      <select
        className="h-8 min-w-0 max-w-52 border border-line bg-surface px-2 text-[12.5px] text-ink outline-none focus:border-signal disabled:opacity-60"
        value={value ?? ""}
        disabled={disabled}
        onChange={(event) => onChange(event.target.value || null)}
      >
        <option value="">{t("Any PC")}</option>
        {devices.map((device) => (
          <option key={device.id} value={device.id}>
            {device.name}
            {device.online ? "" : ` — ${t("offline")}`}
          </option>
        ))}
      </select>
    </label>
  );
}
