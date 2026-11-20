import { useId, useState } from "react";
import type { ReactNode } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useToast } from "@/state/toasts";
import { errorMessage } from "@/lib/http";
import { isNative, native } from "@/lib/native";
import type { ControllerRole, GameControllers, InputDevice, VirtualPadStatus } from "@/lib/types";
import { Badge, Switch } from "@/components/ui/controls";
import { cn } from "@/lib/cn";
import { t, tr } from "@/lib/i18n";
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
 * The virtual gamepad below (#315) is the other half: one pad the game binds,
 * with every physical pad hidden from SDL.
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

      {data ? <VirtualPadSection gameId={gameId} data={data} /> : null}

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

/**
 * Per-game virtual gamepad (#315).
 *
 * Ludex reads pads for its own overlay; this is the other half: one virtual pad
 * the game can bind, with every physical pad hidden from SDL. Linux only, and
 * it needs `/dev/uinput`; the status comes from the backend so the reason is
 * the same sentence Settings → Controllers shows.
 */
function VirtualPadSection({ gameId, data }: { gameId: number; data: GameControllers }) {
  const queryClient = useQueryClient();
  const { push } = useToast();
  const [saving, setSaving] = useState(false);
  const pad = data.virtualPad;
  const available = pad.status === "available";

  const toggle = async (enabled: boolean) => {
    if (saving) return;
    setSaving(true);
    try {
      const next = await native.setGameVirtualPad(gameId, enabled);
      queryClient.setQueryData(qk.gameControllers(gameId), next);
      push({
        title: enabled ? t("Virtual gamepad enabled") : t("Virtual gamepad disabled"),
        description: enabled
          ? t("The next launch creates one virtual pad and hides the physical ones.")
          : t("The next launch keeps the physical pads and the host order."),
        tone: "success",
      });
    } catch (cause) {
      push({
        title: t("Could not save the virtual gamepad option"),
        description: errorMessage(cause),
        tone: "danger",
      });
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="mt-4 border-t border-line pt-3">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <span className="flex flex-wrap items-center gap-2">
            <span className="text-[12px] text-ink-2">{t("Virtual gamepad")}</span>
            <Badge tone={available ? "ok" : pad.status === "missingPermission" ? "signal" : "outline"}>
              {virtualPadStatusLabel(pad.status)}
            </Badge>
          </span>
          <p className="mt-1 max-w-[70ch] text-[11px] leading-relaxed text-ink-3">
            {t(
              "Ludex creates one virtual Xbox-style pad for this game, hides every physical pad from SDL and reads the preferred controller itself.",
            )}
          </p>
          {!available && (
            <p className="mt-1.5 border-l-2 border-signal/60 pl-3 text-[11.5px] leading-relaxed text-ink-2">
              {pad.detail}
            </p>
          )}
          {available && pad.source && (
            <p className="mt-1.5 font-mono text-[10.5px] text-ink-3">
              {tr("Reads {pad}", { pad: pad.source })}
            </p>
          )}
          {pad.warning && (
            <p className="mt-1.5 border-l-2 border-signal/60 pl-3 text-[11.5px] leading-relaxed text-ink-2">
              {pad.warning}
            </p>
          )}
        </div>
        <Switch
          checked={pad.enabled}
          disabled={!available || saving}
          onCheckedChange={(checked) => void toggle(checked)}
          label={t("Virtual gamepad")}
        />
      </div>
      <p className="mt-2 max-w-[70ch] text-[11px] leading-relaxed text-ink-3">
        {t(
          "Enable it for games that bind the wheel instead of the pad. It applies to the next launch; the overlay and Console mode keep using every pad.",
        )}
      </p>
    </div>
  );
}

function virtualPadStatusLabel(status: VirtualPadStatus): string {
  switch (status) {
    case "available":
      return t("Available");
    case "missingPermission":
      return t("Missing permission");
    default:
      return t("Unsupported platform");
  }
}
