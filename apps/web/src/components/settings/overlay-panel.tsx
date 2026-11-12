import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Gamepad2, MonitorPlay } from "lucide-react";
import { isNative, native } from "@/lib/native";
import { errorMessage } from "@/lib/http";
import { clampOverlayOpacity, clampOverlayScale, OVERLAY_TABS } from "@/lib/overlay";
import type { OverlayStatus, OverlayTabId, OverlayTrigger } from "@/lib/types";
import { useToast } from "@/state/toasts";
import { Badge, Button, Checkbox, Segmented } from "@/components/ui/controls";
import { SettingsGroup, SettingsSwitchRow } from "@/components/settings/settings-section";
import { t } from "@/lib/i18n";

/**
 * In-game overlay settings (#144).
 *
 * Every control saves on its own through `set_overlay_settings`, which merges a
 * partial patch and applies it live: no restart, no shared save bar that could
 * drop a change. The same panel is the only place that lists the per-game
 * opt-outs, so an anti-cheat-sensitive title can be turned off without hunting
 * through its page.
 */
export function OverlayPanel() {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const [saving, setSaving] = useState(false);

  const query = useQuery({
    queryKey: ["overlay-status"],
    queryFn: () => native.overlayStatus(),
    enabled: isNative(),
    refetchInterval: 5_000,
  });

  const installed = useQuery({
    queryKey: ["overlay-installed"],
    queryFn: () => native.listInstalled(),
    enabled: isNative(),
    staleTime: 60_000,
  });

  const status = query.data;
  const [opacity, setOpacity] = useState(status?.opacity ?? 0.92);
  const [scale, setScale] = useState(status?.scale ?? 1);

  useEffect(() => {
    if (!status) return;
    setOpacity(status.opacity);
    setScale(status.scale);
  }, [status?.opacity, status?.scale]);

  const commit = async (patch: Parameters<typeof native.setOverlaySettings>[0], label: string) => {
    if (!isNative()) return;
    setSaving(true);
    try {
      await native.setOverlaySettings(patch);
      await queryClient.invalidateQueries({ queryKey: ["overlay-status"] });
    } catch (cause) {
      push({
        title: t("Could not save the overlay setting"),
        description: `${label}: ${errorMessage(cause)}`,
        tone: "danger",
      });
    } finally {
      setSaving(false);
    }
  };

  if (!isNative()) {
    return (
      <SettingsGroup
        title={t("In-game overlay")}
        description={t("The controller overlay belongs to the desktop app. The browser build can browse and download.")}
      >
        <p className="max-w-[70ch] text-[11.5px] leading-relaxed text-ink-3">
          {t("Install Ludex on the gaming PC to open the overlay with Shift+Tab or a controller.")}
        </p>
      </SettingsGroup>
    );
  }

  if (query.isPending) {
    return (
      <SettingsGroup title={t("In-game overlay")}>
        <p className="py-4 text-[13px] text-ink-3">{t("Reading the overlay status…")}</p>
      </SettingsGroup>
    );
  }

  if (!status) {
    return (
      <SettingsGroup title={t("In-game overlay")}>
        <p className="max-w-[70ch] text-[12.5px] leading-relaxed text-danger">
          {t("The overlay status is unavailable. Restart Ludex and try again.")}
        </p>
      </SettingsGroup>
    );
  }

  const enabled = status.enabled;
  const tabs = status.tabs ?? OVERLAY_TABS;
  const disabledNames = new Map((installed.data ?? []).map((game) => [game.gameId, game.title]));

  const toggleTab = (tab: OverlayTabId) => {
    if (tab === "game") return; // the game tab owns Resume/Exit and is never hidden
    const next = tabs.includes(tab) ? tabs.filter((entry) => entry !== tab) : [...tabs, tab];
    void commit({ tabs: next }, t("Tabs"));
  };

  return (
    <SettingsGroup
      title={t("In-game overlay")}
      description={t("A Steam-like panel over the game, opened from the controller. Nothing is injected into the game and the game is never paused.")}
      actions={
        <Button
          size="sm"
          variant="default"
          icon={<MonitorPlay className="size-3.5" />}
          disabled={!enabled}
          onClick={() => {
            void native
              .openOverlay()
              .catch((cause) =>
                push({
                  title: t("Could not open the overlay"),
                  description: errorMessage(cause),
                  tone: "danger",
                }),
              );
          }}
        >
          {t("Preview the overlay")}
        </Button>
      }
    >
      <SettingsSwitchRow
        title={t("Enable the in-game overlay")}
        description={t("Shift+Tab opens it while Ludex is focused; the controller gesture opens it over a borderless-fullscreen game.")}
        checked={enabled}
        onCheckedChange={(next) => void commit({ enabled: next }, t("Enabled"))}
      />

      <div className="border-b border-line py-4">
        <p className="text-[13px] font-medium text-ink">{t("Controller gesture")}</p>
        <p className="mt-1 max-w-[54ch] text-[11.5px] leading-relaxed text-ink-3">
          {t("A single Home press stays with the OS or Steam. The chord is the fallback for pads that hide the Guide button.")}
        </p>
        <div className="mt-3">
          <Segmented<OverlayTrigger>
            label={t("Controller gesture")}
            value={status.trigger}
            onChange={(trigger) => void commit({ trigger }, t("Controller gesture"))}
            options={[
              { value: "doubleHome", label: t("Double Home") },
              { value: "chord", label: t("LB+RB+Start") },
              { value: "keyboard", label: t("Keyboard only") },
            ]}
          />
        </div>
      </div>

      <div className="border-b border-line py-4">
        <div className="flex flex-wrap items-baseline justify-between gap-3">
          <div>
            <p className="text-[13px] font-medium text-ink">{t("Panel opacity")}</p>
            <p className="mt-1 text-[11.5px] text-ink-3">{t("The game stays visible behind the panel.")}</p>
          </div>
          <span className="font-mono text-[12px] tabular-nums text-ink-2">{Math.round(opacity * 100)}%</span>
        </div>
        <input
          type="range"
          min={35}
          max={100}
          step={1}
          value={Math.round(opacity * 100)}
          aria-label={t("Panel opacity")}
          onChange={(event) => setOpacity(clampOverlayOpacity(Number(event.target.value) / 100))}
          onPointerUp={() => void commit({ opacity }, t("Panel opacity"))}
          onKeyUp={() => void commit({ opacity }, t("Panel opacity"))}
          onBlur={() => void commit({ opacity }, t("Panel opacity"))}
          className="mt-3 w-full max-w-[420px] accent-signal"
        />
      </div>

      <div className="border-b border-line py-4">
        <div className="flex flex-wrap items-baseline justify-between gap-3">
          <div>
            <p className="text-[13px] font-medium text-ink">{t("Panel scale")}</p>
            <p className="mt-1 text-[11.5px] text-ink-3">{t("Size of the panel relative to the screen.")}</p>
          </div>
          <span className="font-mono text-[12px] tabular-nums text-ink-2">{scale.toFixed(2)}×</span>
        </div>
        <input
          type="range"
          min={60}
          max={130}
          step={1}
          value={Math.round(scale * 100)}
          aria-label={t("Panel scale")}
          onChange={(event) => setScale(clampOverlayScale(Number(event.target.value) / 100))}
          onPointerUp={() => void commit({ scale }, t("Panel scale"))}
          onKeyUp={() => void commit({ scale }, t("Panel scale"))}
          onBlur={() => void commit({ scale }, t("Panel scale"))}
          className="mt-3 w-full max-w-[420px] accent-signal"
        />
      </div>

      <div className="border-b border-line py-4">
        <p className="text-[13px] font-medium text-ink">{t("Tabs")}</p>
        <p className="mt-1 text-[11.5px] text-ink-3">
          {t("The game tab is always shown; it holds Resume and Exit.")}
        </p>
        <div className="mt-3 flex flex-wrap gap-x-6 gap-y-3">
          {OVERLAY_TABS.map((tab) => {
            const label = overlayTabLabel(tab);
            const checked = tab === "game" || tabs.includes(tab);
            return (
              <label key={tab} className="flex items-center gap-2 text-[12.5px] text-ink-2">
                <Checkbox
                  checked={checked}
                  onCheckedChange={() => toggleTab(tab)}
                  label={label}
                />
                {label}
              </label>
            );
          })}
        </div>
      </div>

      <div className="border-b border-line py-4">
        <p className="text-[13px] font-medium text-ink">{t("Disabled for these games")}</p>
        <p className="mt-1 max-w-[60ch] text-[11.5px] leading-relaxed text-ink-3">
          {t("Use this for anti-cheat-sensitive titles. The overlay also stays off for a game whose page opted out.")}
        </p>
        {status.disabledGames.length === 0 ? (
          <p className="mt-3 text-[12px] text-ink-3">{t("No game is opted out.")}</p>
        ) : (
          <ul className="mt-3 flex flex-col gap-1.5">
            {status.disabledGames.map((gameId) => (
              <li key={gameId} className="flex items-center justify-between gap-4 border-b border-line pb-1.5">
                <span className="truncate text-[12.5px] text-ink">
                  {disabledNames.get(gameId) ?? t("Game {id}", { id: String(gameId) })}
                </span>
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() =>
                    void commit(
                      { disabledGames: status.disabledGames.filter((entry) => entry !== gameId) },
                      t("Disabled for these games"),
                    )
                  }
                >
                  {t("Enable again")}
                </Button>
              </li>
            ))}
          </ul>
        )}
      </div>

      <ListenerRow status={status} saving={saving} />

      <p className="mt-4 max-w-[70ch] text-[11.5px] leading-relaxed text-ink-3">{status.platformNote}</p>
    </SettingsGroup>
  );
}

function overlayTabLabel(tab: OverlayTabId): string {
  switch (tab) {
    case "game":
      return t("Now playing");
    case "library":
      return t("Library");
    case "downloads":
      return t("Downloads");
    case "achievements":
      return t("Achievements");
  }
}

function ListenerRow({ status, saving }: { status: OverlayStatus; saving: boolean }) {
  const listener = status.listener;
  const active = listener.controllers.find((pad) => pad.active) ?? listener.controllers[0];
  return (
    <div className="py-4">
      <p className="text-[13px] font-medium text-ink">{t("Controller listener")}</p>
      <p className="mt-1 max-w-[60ch] text-[11.5px] leading-relaxed text-ink-3">
        {listener.available
          ? t("The global listener reads controllers even while a game is focused.")
          : (listener.reason ?? t("The global listener is not available on this system."))}
      </p>
      <div className="mt-3 flex flex-wrap items-center gap-3">
        <Badge tone={listener.available ? "ok" : "danger"}>
          {listener.available ? listener.backend : t("unavailable")}
        </Badge>
        <span className="font-mono text-[11px] uppercase tracking-[0.12em] text-ink-3">{listener.platform}</span>
        {active ? (
          <span className="flex items-center gap-2 text-[12px] text-ink-2">
            <Gamepad2 className="size-3.5" />
            {active.name}
            {active.battery !== null ? ` · ${active.battery}%${active.charging ? "+" : ""}` : ""}
          </span>
        ) : (
          <span className="text-[12px] text-ink-3">
            {listener.available ? t("No controller detected.") : t("No controller is being read.")}
          </span>
        )}
        {saving && <span className="text-[11px] text-ink-3">{t("Saving…")}</span>}
      </div>
    </div>
  );
}
