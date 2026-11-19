import { useQuery } from "@tanstack/react-query";
import { Gamepad2, RefreshCw } from "lucide-react";
import { errorMessage } from "@/lib/http";
import { isNative, native } from "@/lib/native";
import type { ControllerRole, InputDevice } from "@/lib/types";
import { Badge, Button } from "@/components/ui/controls";
import { t } from "@/lib/i18n";
import { qk } from "@/lib/query-keys";

/**
 * Launch-time controllers (#314).
 *
 * The browser Gamepad API above shows what the *library* sees; this panel shows
 * what a game launched by Ludex sees: the joysticks read from
 * `/proc/bus/input/devices` and `/dev/input/js*`, in enumeration order. A wheel
 * in the first row is exactly why a game can bind the wheel instead of the pad.
 *
 * Read-only: the per-game choice lives in the game's launch options.
 */
export function ControllerDiagnostics() {
  const query = useQuery({
    queryKey: qk.controllerInventory,
    queryFn: () => native.controllerInventory(),
    enabled: isNative(),
    staleTime: 15_000,
    retry: false,
  });
  if (!isNative()) return null;

  const inventory = query.data;
  const joysticks: InputDevice[] = inventory?.devices.filter((device) => device.isJoystick) ?? [];

  return (
    <section
      className="mt-4 rounded-[3px] border border-line bg-bg-2/40 p-4 sm:p-5"
      aria-label={t("Launch-time controllers")}
    >
      <header className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h4 className="flex items-center gap-2 font-mono text-[11px] uppercase tracking-[0.12em] text-ink-2">
            <Gamepad2 className="size-3.5" aria-hidden="true" /> {t("Launch-time controllers")}
          </h4>
          <p className="mt-1 max-w-[68ch] text-[12.5px] leading-relaxed text-ink-3">
            {t(
              "The joysticks a game launched by Ludex will see, in the order this machine enumerates them. Pick a preferred controller per game in its launch options.",
            )}
          </p>
        </div>
        <Button
          variant="ghost"
          size="sm"
          icon={<RefreshCw className={query.isFetching ? "size-3.5 animate-spin" : "size-3.5"} />}
          onClick={() => void query.refetch()}
        >
          {t("Refresh")}
        </Button>
      </header>

      {query.isError ? (
        <p className="mt-3 border-l-2 border-danger/60 pl-3 text-[12px] leading-relaxed text-ink-2">
          {errorMessage(query.error)}
        </p>
      ) : !inventory ? (
        <p className="mt-3 text-[12px] text-ink-3">{t("Reading the input devices…")}</p>
      ) : !inventory.available ? (
        <p className="mt-3 border-l-2 border-line pl-3 text-[12px] leading-relaxed text-ink-3">
          {inventory.note ?? t("The input device list is unavailable on this system.")}
        </p>
      ) : joysticks.length === 0 ? (
        <p className="mt-3 text-[12px] text-ink-3">{t("No joystick detected.")}</p>
      ) : (
        <ul className="mt-3 divide-y divide-line border-t border-line">
          {joysticks.map((device) => (
            <li
              key={`${device.name}-${device.deviceId ?? "none"}-${device.jsIndex ?? device.eventIndex ?? 0}`}
              className="grid items-center gap-2 py-2.5 sm:grid-cols-[auto_1fr_auto_auto]"
            >
              <Badge tone={device.role === "wheel" ? "signal" : "outline"}>
                {roleLabel(device.role)}
              </Badge>
              <span className="min-w-0 truncate text-[12.5px] text-ink">{device.name}</span>
              <span className="font-mono text-[11px] text-ink-3">
                {device.deviceId ?? t("no vendor:product")}
              </span>
              <span className="flex items-center justify-end gap-2">
                <span className="font-mono text-[11px] text-ink-3">
                  {device.jsIndex !== null ? `js${device.jsIndex}` : device.eventIndex !== null ? `event${device.eventIndex}` : "—"}
                </span>
                <Badge tone={device.connected ? "ok" : "danger"}>
                  {device.connected ? t("Connected") : t("Not connected")}
                </Badge>
              </span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function roleLabel(role: ControllerRole): string {
  switch (role) {
    case "wheel":
      return t("Steering wheel");
    case "gamepad":
      return t("Gamepad");
    case "joystick":
      return t("Joystick");
    case "mouse":
      return t("Mouse");
    case "keyboard":
      return t("Keyboard");
    default:
      return t("Other device");
  }
}
