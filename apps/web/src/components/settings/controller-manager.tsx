import { useMemo } from "react";
import { useQuery } from "@tanstack/react-query";
import { Gamepad2, RotateCcw } from "lucide-react";
import { api } from "@/lib/api";
import {
  GLYPH_THEMES,
  identityLabel,
  normalizePadName,
  shortFingerprint,
  type GlyphTheme,
} from "@/lib/controller-identity";
import {
  sourceFromId,
  sourceId,
  type ControllerDeviceEntry,
  type ControllerDevicesStore,
} from "@/lib/controller-devices";
import { presetProfile } from "@/lib/controller-profiles";
import { isNative, native } from "@/lib/native";
import { normalizeOverlayStatus } from "@/lib/overlay";
import { useConnectedPads, useControllerDevices, type ConnectedPad } from "@/lib/use-controller-devices";
import { profileOptions, type ProfileOption } from "@/components/big-picture/controller";
import { Badge, Button, Switch } from "@/components/ui/controls";
import { Select } from "@/components/ui/select";
import { t } from "@/lib/i18n";

/* --------------------------------------------------------------------------
   The controller manager (#230).

   A short list at the top of Settings → Profiles → Controllers: every pad the
   browser currently exposes, keyed by fingerprint (never by connection order),
   with the profile it will use, its glyph theme and the A/B swap. The profile
   editor below stays the place to tune deadzone, curve and the button map.

   The gamepad API cannot tell two identical pads apart across sessions, so two
   rows can share one fingerprint and one profile while both are connected. The
   note under the heading says so instead of pretending otherwise.
   -------------------------------------------------------------------------- */

export function ControllerManager() {
  const pads = useConnectedPads();
  const [devices, store] = useControllerDevices();
  const profiles = useQuery({
    queryKey: ["controller-profiles"],
    queryFn: () => api.controllerProfiles(),
    staleTime: 5 * 60_000,
  });
  const fallbackProfile = useMemo(() => presetProfile("xbox"), []);
  const options = useMemo(
    () => profileOptions(profiles.data, fallbackProfile),
    [profiles.data, fallbackProfile],
  );
  const battery = useNativeBattery();

  const connected = new Set(pads.map((pad) => pad.identity.fingerprint));
  const remembered = Object.values(devices.pads).filter(
    (entry) => !connected.has(entry.identity.fingerprint),
  );

  return (
    <section
      className="rounded-[3px] border border-line bg-bg-2/40 p-4 sm:p-5"
      aria-label={t("Connected pads")}
    >
      <header className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h4 className="flex items-center gap-2 font-mono text-[11px] uppercase tracking-[0.12em] text-ink-2">
            <Gamepad2 className="size-3.5" aria-hidden="true" /> {t("Connected pads")}
          </h4>
          <p className="mt-1 max-w-[62ch] text-[12.5px] leading-relaxed text-ink-3">
            {t("Profiles are stored on this device by pad fingerprint. Two identical pads share one profile.")}
          </p>
        </div>
        <Badge tone={pads.length > 0 ? "signal" : "outline"}>
          {pads.length > 0 ? pads.length : t("No controller detected")}
        </Badge>
      </header>

      {pads.length === 0 ? (
        <p className="mt-4 rounded-[2px] border border-line/70 bg-bg/40 px-3 py-2 text-[12px] text-ink-3">
          {t("Connect a controller and press a button to see it here.")}
        </p>
      ) : (
        <ul className="mt-4 divide-y divide-line/60 border-y border-line/60">
          {pads.map((pad) => (
            <ControllerPadRow
              key={`${pad.index}:${pad.identity.fingerprint}`}
              pad={pad}
              entry={devices.pads[pad.identity.fingerprint] ?? null}
              options={options}
              store={store}
              last={pad.identity.fingerprint === devices.last}
              battery={battery.get(normalizePadName(pad.id)) ?? null}
            />
          ))}
        </ul>
      )}

      {remembered.length > 0 && (
        <div className="mt-4">
          <h5 className="mb-2 font-mono text-[10px] uppercase tracking-[0.12em] text-ink-3">
            {t("Remembered pads")}
          </h5>
          <ul className="divide-y divide-line/60 border-y border-line/60">
            {remembered.map((entry) => (
              <li key={entry.identity.fingerprint} className="flex flex-wrap items-center gap-2 py-2">
                <span className="text-[12px] text-ink-2">{entry.label}</span>
                <span className="font-mono text-[11px] text-ink-3">
                  {shortFingerprint(entry.identity.fingerprint)}
                </span>
                <Badge tone="outline">{t("Not connected")}</Badge>
                <Button
                  size="sm"
                  variant="ghost"
                  className="ms-auto"
                  icon={<RotateCcw className="size-3.5" />}
                  onClick={() => store.remove(entry.identity.fingerprint)}
                >
                  {t("Reset to automatic")}
                </Button>
              </li>
            ))}
          </ul>
        </div>
      )}
    </section>
  );
}

function ControllerPadRow({
  pad,
  entry,
  options,
  store,
  last,
  battery,
}: {
  pad: ConnectedPad;
  entry: ControllerDeviceEntry | null;
  options: readonly ProfileOption[];
  store: ControllerDevicesStore;
  last: boolean;
  battery: { level: number; charging: boolean } | null;
}) {
  // The same rule the fallback uses: a layout we cannot trust.
  const generic = pad.mapping !== "standard" || pad.buttons < 12;
  const label = identityLabel(pad.identity);
  return (
    <li className="grid gap-3 py-3 sm:grid-cols-[minmax(0,1fr)_minmax(0,18rem)]">
      <div className="min-w-0">
        <div className="flex flex-wrap items-center gap-2">
          <span className="truncate text-[13px] text-ink">{label}</span>
          <Badge tone="signal">{t("Connected")}</Badge>
          {last && <Badge tone="outline">{t("Last used")}</Badge>}
          {generic && <Badge tone="danger">{t("Generic — remap if a button is wrong")}</Badge>}
        </div>
        <p className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 font-mono text-[10.5px] text-ink-3">
          <span title={t("Fingerprint")}>{shortFingerprint(pad.identity.fingerprint)}</span>
          <span className="max-w-[36ch] truncate normal-case tracking-normal" title={pad.id}>
            {pad.id}
          </span>
        </p>
        <p className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 font-mono text-[10.5px] uppercase tracking-[0.08em] text-ink-3">
          <span>{pad.mapping === "standard" ? t("Standard") : t("Mapping")}</span>
          <span>
            {t("Buttons")}: {pad.buttons}
          </span>
          <span>
            {t("Axes")}: {pad.axes}
          </span>
          {battery && (
            <span className="tabular-nums">
              {t("Battery")}: {battery.level}%{battery.charging ? "+" : ""}
            </span>
          )}
        </p>
      </div>

      <div className="grid gap-2">
        <Select
          label={t("Profile")}
          value={entry ? sourceId(entry.source) : "auto"}
          onChange={(value) => store.assign(pad.identity, { source: sourceFromId(value) })}
          options={options.map((option) => ({ value: option.id, label: option.label }))}
        />
        <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
          <div className="min-w-[9.5rem] flex-1">
            <Select
              label={t("Glyph theme")}
              value={entry?.glyphTheme ?? "auto"}
              onChange={(value) => store.assign(pad.identity, { glyphTheme: value as GlyphTheme })}
              options={GLYPH_THEMES.map((theme) => ({ value: theme, label: glyphThemeLabel(theme) }))}
            />
          </div>
          <Switch
            checked={Boolean(entry?.swapAB)}
            onCheckedChange={(swapAB) => store.assign(pad.identity, { swapAB })}
            label={t("Swap A and B")}
          />
          <Button
            size="sm"
            variant="ghost"
            icon={<RotateCcw className="size-3.5" />}
            onClick={() => store.remove(pad.identity.fingerprint)}
            disabled={!entry}
          >
            {t("Reset to automatic")}
          </Button>
        </div>
      </div>
    </li>
  );
}

function glyphThemeLabel(theme: GlyphTheme): string {
  switch (theme) {
    case "auto":
      return t("Automatic");
    case "xbox":
      return "Xbox";
    case "playstation":
      return "PlayStation";
    case "switch":
      return "Switch";
    case "steamdeck":
      return "Steam Deck";
    case "numbers":
      return t("Numbers");
  }
}

/**
 * Battery is not in the Gamepad API; the native overlay listener reads it from
 * gilrs (when the bundle supports it), so the manager borrows that snapshot and
 * matches by normalised pad name.
 */
function useNativeBattery(): Map<string, { level: number; charging: boolean }> {
  const enabled = isNative();
  const query = useQuery({
    queryKey: ["overlay-status"],
    queryFn: () => native.overlayStatus(),
    enabled,
    staleTime: 5_000,
    refetchInterval: enabled ? 10_000 : false,
  });
  return useMemo(() => {
    const map = new Map<string, { level: number; charging: boolean }>();
    const status = query.data ? normalizeOverlayStatus(query.data) : null;
    for (const pad of status?.listener.controllers ?? []) {
      if (pad.battery === null) continue;
      map.set(normalizePadName(pad.name), { level: pad.battery, charging: pad.charging });
    }
    return map;
  }, [query.data]);
}
