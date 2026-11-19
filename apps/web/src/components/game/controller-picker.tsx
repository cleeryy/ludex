import { useId, useState } from "react";
import type { ReactNode } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useToast } from "@/state/toasts";
import { errorMessage } from "@/lib/http";
import { isNative, native } from "@/lib/native";
import type { ControllerRole, InputDevice } from "@/lib/types";
import { Badge } from "@/components/ui/controls";
import { cn } from "@/lib/cn";
import { t } from "@/lib/i18n";
import { qk } from "@/lib/query-keys";

/**
 * Per-game preferred controller (#314).
 *
 * A wheel enumerated as controller 0 is what a Proton game binds before the
 * pad. Picking the pad here makes Ludex hide the *other* joysticks from SDL for
 * this game only; the overlay and Console mode keep using every pad.
 *
 * The choice is stored on this device and resolved again at every launch: an
 * unplugged pad leaves the environment untouched instead of hiding the wheel.
 * A true virtual pad is out of scope (#315).
 */
export function ControllerPicker({ gameId }: { gameId: number }) {
  const queryClient = useQueryClient();
  const { push } = useToast();
  const group = useId();
  const [saving, setSaving] = useState(false);

  const query = useQuery({
    queryKey: qk.gameControllers(gameId),
    queryFn: () => native.gameControllers(gameId),
    enabled: isNative(),
    staleTime: 15_000,
    retry: false,
  });
  if (!isNative()) return null;

  const data = query.data;
  const joysticks: InputDevice[] = data?.inventory.devices.filter((device) => device.isJoystick) ?? [];
  const selected = data?.preferred?.deviceId ?? "";
  const env = Object.entries(data?.launch.env ?? {});

  const choose = async (deviceId: string | null) => {
    if (saving) return;
    setSaving(true);
    try {
      const next = await native.setGameController(gameId, deviceId);
      queryClient.setQueryData(qk.gameControllers(gameId), next);
      push({
        title: deviceId ? t("Preferred controller saved") : t("Controller order reset"),
        description: deviceId
          ? t("The other joysticks are hidden from this game at its next launch.")
          : t("The game follows the host order again."),
        tone: "success",
      });
    } catch (cause) {
      push({
        title: t("Could not save the preferred controller"),
        description: errorMessage(cause),
        tone: "danger",
      });
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="mt-6 border-t border-line pt-4">
      <div className="flex flex-wrap items-baseline justify-between gap-3">
        <p className="text-[13px] font-medium text-ink">{t("Controllers")}</p>
        <Badge tone={selected ? "signal" : "outline"}>
          {selected ? t("Preferred") : t("Host order")}
        </Badge>
      </div>
      <p className="mt-1 max-w-[70ch] text-[11.5px] leading-relaxed text-ink-3">
        {t(
          "Games read controller 0 first. When a wheel or an extra pad comes first, the game can bind the wrong device. Pick the pad below: Ludex hides the other joysticks from SDL for this game only.",
        )}
      </p>

      {query.isError ? (
        <p className="mt-3 border-l-2 border-danger/60 pl-3 text-[12px] leading-relaxed text-ink-2">
          {errorMessage(query.error)}
        </p>
      ) : !data ? (
        <p className="mt-3 text-[12px] text-ink-3">{t("Reading the input devices…")}</p>
      ) : (
        <fieldset className="mt-3" disabled={saving}>
          <legend className="sr-only">{t("Preferred controller")}</legend>
          <div className="flex flex-col gap-1.5">
            <Choice
              group={group}
              checked={selected === ""}
              title={t("Automatic (host order)")}
              description={t("The game sees every joystick in the order above.")}
              onChange={() => void choose(null)}
            />
            {joysticks.map((device) => {
              const selectable = Boolean(device.deviceId) && device.connected;
              return (
                <Choice
                  key={`${device.deviceId ?? "none"}-${device.jsIndex ?? device.eventIndex ?? 0}-${device.name}`}
                  group={group}
                  checked={selected !== "" && selected === device.deviceId}
                  disabled={!selectable}
                  title={device.name}
                  description={
                    !device.deviceId
                      ? t("No vendor:product pair: SDL cannot exclude it.")
                      : !device.connected
                        ? t("Not connected.")
                        : t("The other joysticks are hidden so this one becomes controller 0.")
                  }
                  meta={
                    <span className="flex flex-wrap items-center gap-2">
                      <Badge tone={device.role === "wheel" ? "signal" : "outline"}>
                        {roleLabel(device.role)}
                      </Badge>
                      <span className="font-mono text-[10.5px] text-ink-3">
                        {device.deviceId ?? "—"}
                      </span>
                      <span className="font-mono text-[10.5px] text-ink-3">
                        {device.jsIndex !== null
                          ? `js${device.jsIndex}`
                          : device.eventIndex !== null
                            ? `event${device.eventIndex}`
                            : ""}
                      </span>
                    </span>
                  }
                  onChange={() => void choose(device.deviceId)}
                />
              );
            })}
          </div>
        </fieldset>
      )}

      {!data?.inventory.available && data ? (
        <p className="mt-3 border-l-2 border-line pl-3 text-[11.5px] leading-relaxed text-ink-3">
          {data.inventory.note ?? t("The input device list is unavailable on this system.")}
        </p>
      ) : null}

      {joysticks.length === 0 && data?.inventory.available ? (
        <p className="mt-3 text-[11.5px] text-ink-3">{t("No joystick detected.")}</p>
      ) : null}

      {data?.launch.warning ? (
        <p className="mt-3 border-l-2 border-signal/60 pl-3 text-[11.5px] leading-relaxed text-ink-2">
          {data.launch.warning}
        </p>
      ) : null}

      {env.length > 0 ? (
        <div className="mt-4">
          <p className="text-[11px] font-medium text-ink-2">{t("Launch environment")}</p>
          <dl className="mt-1.5 flex flex-col gap-1">
            {env.map(([key, value]) => (
              <div key={key} className="flex flex-wrap items-baseline gap-1.5 font-mono text-[10.5px]">
                <dt className="text-ink-3">{key}</dt>
                <dd className="break-all text-ink-2">= {value}</dd>
              </div>
            ))}
          </dl>
          <p className="mt-1.5 max-w-[70ch] text-[11px] leading-relaxed text-ink-3">
            {t(
              "Wine and Proton receive these through SDL. A per-game environment override with the same name still wins.",
            )}
          </p>
        </div>
      ) : null}

      <p className="mt-3 max-w-[70ch] text-[11px] leading-relaxed text-ink-3">
        {t("Applies from the next launch. The overlay and Console mode keep using every pad.")}
      </p>
    </div>
  );
}

function Choice({
  group,
  checked,
  disabled = false,
  title,
  description,
  meta,
  onChange,
}: {
  group: string;
  checked: boolean;
  disabled?: boolean;
  title: string;
  description: string;
  meta?: ReactNode;
  onChange: () => void;
}) {
  return (
    <label
      className={cn(
        "flex cursor-pointer items-start gap-3 rounded-[3px] border px-3 py-2.5 transition-colors",
        checked ? "border-signal/50 bg-signal/12" : "border-line hover:border-line-2",
        disabled && "cursor-not-allowed opacity-55",
      )}
    >
      <input
        type="radio"
        name={group}
        checked={checked}
        disabled={disabled}
        onChange={onChange}
        className="mt-0.5 size-3.5 accent-[var(--signal)]"
      />
      <span className="min-w-0 flex-1">
        <span className="flex flex-wrap items-center justify-between gap-2">
          <span className="truncate text-[12.5px] text-ink">{title}</span>
          {meta}
        </span>
        <span className="mt-0.5 block text-[11px] leading-relaxed text-ink-3">{description}</span>
      </span>
    </label>
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
