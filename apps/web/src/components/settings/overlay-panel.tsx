import { useEffect, useState, type ReactNode } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Gamepad2, MonitorPlay } from "lucide-react";
import { isNative, native } from "@/lib/native";
import { errorMessage } from "@/lib/http";
import {
  clampOverlayOpacity,
  clampOverlayScale,
  doubleHomeAvailable,
  normalizeOverlayStatus,
  OVERLAY_TABS,
  overlayLastButtonState,
  overlayWindowOutcome,
  selectedTrigger,
} from "@/lib/overlay";
import type {
  OverlayCheck,
  OverlayStatus,
  OverlayTabId,
  OverlayTrigger,
  OverlayTriggerEvent,
  OverlayWindowResult,
} from "@/lib/types";
import { useToast } from "@/state/toasts";
import { Badge, Button, Checkbox, Segmented } from "@/components/ui/controls";
import { SettingsGroup, SettingsSwitchRow } from "@/components/settings/settings-section";
import { t } from "@/lib/i18n";
import { qk } from "@/lib/query-keys";

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
    queryKey: qk.overlay.status,
    queryFn: () => native.overlayStatus(),
    enabled: isNative(),
    refetchInterval: 5_000,
  });

  const installed = useQuery({
    queryKey: qk.overlay.installed,
    queryFn: () => native.listInstalled(),
    enabled: isNative(),
    staleTime: 60_000,
  });

  const status = query.data ? normalizeOverlayStatus(query.data) : null;
  const [opacity, setOpacity] = useState(status?.opacity ?? 0.92);
  const [scale, setScale] = useState(status?.scale ?? 1);
  /** Facts from the last forced window test (#228); null until the user asks. */
  const [windowTest, setWindowTest] = useState<OverlayWindowResult | null>(null);
  const [testing, setTesting] = useState(false);

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
      await queryClient.invalidateQueries({ queryKey: qk.overlay.status });
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

  /**
   * Test button (#228): ask the desktop for the floating window even though the
   * main window is focused, then render what actually happened. A toast is not
   * proof; the window facts are. An older desktop ignores the flag and returns
   * only the status, which the panel reports as "not reported".
   */
  const testOverlay = async () => {
    if (!isNative()) return;
    setTesting(true);
    try {
      const opened = await native.openOverlay({ forceWindow: true });
      setWindowTest({
        presentation: opened.presentation,
        windowCreated: opened.windowCreated,
        windowVisible: opened.windowVisible,
        windowError: opened.windowError ?? null,
        monitor: opened.monitor ?? null,
      });
      await queryClient.invalidateQueries({ queryKey: qk.overlay.status });
    } catch (cause) {
      setWindowTest({ windowCreated: false, windowVisible: false, windowError: errorMessage(cause) });
      push({
        title: t("Could not open the overlay"),
        description: errorMessage(cause),
        tone: "danger",
      });
    } finally {
      setTesting(false);
    }
  };

  /** Open the overlay the normal way: in-app while Ludex is focused. */
  const openNow = async () => {
    if (!isNative()) return;
    try {
      const opened = await native.openOverlay();
      const detail = opened.lastTrigger?.detail;
      push({
        title: t("Overlay opened"),
        description:
          opened.presentation === "window"
            ? t("Opened in the floating window.")
            : (detail ?? t("Opened inside Ludex.")),
        tone: "success",
      });
      await queryClient.invalidateQueries({ queryKey: qk.overlay.status });
    } catch (cause) {
      push({
        title: t("Could not open the overlay"),
        description: errorMessage(cause),
        tone: "danger",
      });
    }
  };

  if (!isNative()) {
    return (
      <SettingsGroup
        title={t("In-game overlay")}
        description={t("The controller overlay belongs to the desktop app. The browser build can browse and download.")}
      >
        <p className="max-w-[70ch] text-meta leading-relaxed text-ink-3">
          {t("Install Ludex on the gaming PC to open the overlay with Shift+Tab or a controller.")}
        </p>
      </SettingsGroup>
    );
  }

  if (query.isPending) {
    return (
      <SettingsGroup title={t("In-game overlay")}>
        <p className="py-4 text-body text-ink-3">{t("Reading the overlay status…")}</p>
      </SettingsGroup>
    );
  }

  if (!status) {
    return (
      <SettingsGroup title={t("In-game overlay")}>
        <p className="max-w-[70ch] text-label leading-relaxed text-danger">
          {t("The overlay status is unavailable. Restart Ludex and try again.")}
        </p>
      </SettingsGroup>
    );
  }

  const enabled = status.enabled;
  const tabs = status.tabs ?? OVERLAY_TABS;
  const disabledNames = new Map((installed.data ?? []).map((game) => [game.gameId, game.title]));
  const guideAvailable = doubleHomeAvailable(status.capabilities);
  const trigger = selectedTrigger(status);
  const triggerOptions: { value: OverlayTrigger; label: string }[] = [
    ...(guideAvailable ? [{ value: "doubleHome" as const, label: t("Double Home") }] : []),
    { value: "chord", label: t("LB+RB+Start") },
    { value: "keyboard", label: t("Keyboard only") },
  ];

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
          disabled={!enabled || testing}
          onClick={() => void testOverlay()}
        >
          {testing ? t("Sending…") : t("Send test overlay")}
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
        <p className="text-body font-medium text-ink">{t("Controller gesture")}</p>
        <p className="mt-1 max-w-[54ch] text-meta leading-relaxed text-ink-3">
          {t("A single Home press stays with the OS or Steam. The chord is the fallback for pads that hide the Guide button.")}
        </p>
        {!guideAvailable && (
          <p className="mt-2 max-w-[58ch] text-meta leading-relaxed text-signal-text">
            {t("This system does not deliver the Guide button to apps, so Double Home cannot fire. LB+RB+Start is the default and opens the overlay.")}
          </p>
        )}
        <div className="mt-3">
          <Segmented<OverlayTrigger>
            label={t("Controller gesture")}
            value={trigger}
            onChange={(next) => void commit({ trigger: next }, t("Controller gesture"))}
            options={triggerOptions}
          />
        </div>
      </div>

      <div className="border-b border-line py-4">
        <div className="flex flex-wrap items-baseline justify-between gap-3">
          <div>
            <p className="text-body font-medium text-ink">{t("Panel opacity")}</p>
            <p className="mt-1 text-meta text-ink-3">{t("The game stays visible behind the panel.")}</p>
          </div>
          <span className="font-mono text-label tabular-nums text-ink-2">{Math.round(opacity * 100)}%</span>
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
            <p className="text-body font-medium text-ink">{t("Panel scale")}</p>
            <p className="mt-1 text-meta text-ink-3">{t("Size of the panel relative to the screen.")}</p>
          </div>
          <span className="font-mono text-label tabular-nums text-ink-2">{scale.toFixed(2)}×</span>
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
        <p className="text-body font-medium text-ink">{t("Tabs")}</p>
        <p className="mt-1 text-meta text-ink-3">
          {t("The game tab is always shown; it holds Resume and Exit.")}
        </p>
        <div className="mt-3 flex flex-wrap gap-x-6 gap-y-3">
          {OVERLAY_TABS.map((tab) => {
            const label = overlayTabLabel(tab);
            const checked = tab === "game" || tabs.includes(tab);
            return (
              <label key={tab} className="flex items-center gap-2 text-label text-ink-2">
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
        <p className="text-body font-medium text-ink">{t("Disabled for these games")}</p>
        <p className="mt-1 max-w-[60ch] text-meta leading-relaxed text-ink-3">
          {t("Use this for anti-cheat-sensitive titles. The overlay also stays off for a game whose page opted out.")}
        </p>
        {status.disabledGames.length === 0 ? (
          <p className="mt-3 text-label text-ink-3">{t("No game is opted out.")}</p>
        ) : (
          <ul className="mt-3 flex flex-col gap-1.5">
            {status.disabledGames.map((gameId) => (
              <li key={gameId} className="flex items-center justify-between gap-4 border-b border-line pb-1.5">
                <span className="truncate text-label text-ink">
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

      <DiagnosticsChecklist
        status={status}
        saving={saving}
        windowTest={windowTest}
        onOpenNow={() => void openNow()}
      />

      <p className="mt-4 max-w-[70ch] text-meta leading-relaxed text-ink-3">{status.platformNote}</p>
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

/**
 * The checklist that answers "why does the overlay not open".
 *
 * The desktop reports one stable check id per reason; the labels live here as
 * literal `t()` keys so every locale gets the explanation, with the backend's
 * English message as the fallback for an id this bundle does not know yet.
 */
function DiagnosticsChecklist({
  status,
  saving,
  windowTest,
  onOpenNow,
}: {
  status: OverlayStatus;
  saving: boolean;
  /** Facts from a forced window test, or null before the user asks (#228). */
  windowTest: OverlayWindowResult | null;
  onOpenNow: () => void;
}) {
  const diagnostics = status.diagnostics;
  const blocking = diagnostics.checks.some((check) => check.level === "blocking");
  const listener = status.listener;
  const active = listener.controllers.find((pad) => pad.active) ?? listener.controllers[0];
  const lastButton = overlayLastButtonState(status);

  return (
    <div className="border-b border-line py-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <p className="text-body font-medium text-ink">{t("Why the overlay does not open")}</p>
        <div className="flex items-center gap-2">
          {saving && <span className="text-meta text-ink-3">{t("Saving…")}</span>}
          <Badge tone={diagnostics.summary === "ready" ? "ok" : blocking ? "danger" : "signal"}>
            {diagnostics.summary === "ready" ? t("Ready") : t("Needs attention")}
          </Badge>
        </div>
      </div>
      <p className="mt-1 max-w-[62ch] text-meta leading-relaxed text-ink-3">
        {diagnostics.summary === "ready"
          ? t("Everything the controller overlay needs is ready.")
          : t("Fix the first item below, then press the configured gesture again.")}
      </p>

      <ul className="mt-3 flex flex-col gap-2">
        {diagnostics.checks.map((check) => (
          <li key={check.id} className="flex items-start gap-2">
            <span
              className={`mt-[6px] size-1.5 shrink-0 rounded-full ${checkDot(check)}`}
              aria-hidden="true"
            />
            <span className="text-label leading-relaxed text-ink-2">
              {overlayCheckLabel(check)}
              {check.id === "listener-failed" && (
                <span className="text-ink-3"> · {check.message}</span>
              )}
            </span>
          </li>
        ))}
      </ul>

      <div className="mt-4 grid gap-3 sm:grid-cols-2">
        <DiagnosticFact label={t("Effective gesture")}>
          {effectiveGestureLabel(status)}
          {!status.triggerStatus.deliverable && (
            <span className="text-ink-3"> · {t("not available")}</span>
          )}
        </DiagnosticFact>
        <DiagnosticFact label={t("Listener")}>
          {listener.backend} · {listener.running ? t("Running") : t("Stopped")} ·{" "}
          {listener.available ? t("Available") : t("Unavailable")}
        </DiagnosticFact>
        <DiagnosticFact label={t("Controllers")}>
          <span className="flex items-center gap-2">
            <Gamepad2 className="size-3.5" />
            {listener.controllers.length}
            <span className="text-ink-3">
              {active ? `${active.name} #${active.id}` : t("No controller detected.")}
            </span>
            {active?.battery !== null && active?.battery !== undefined
              ? ` · ${active.battery}%${active.charging ? "+" : ""}`
              : ""}
          </span>
        </DiagnosticFact>
        <DiagnosticFact label={t("Last button")}>
          {lastButton === "never" ? (
            t("No pad button received yet.")
          ) : lastButton === "seen" && status.lastButton ? (
            <span>
              {status.lastButton.button}
              {status.lastButton.pad ? <span className="text-ink-3"> · {status.lastButton.pad}</span> : null}
              {" · "}
              {formatEventTime(status.lastButton.at)}
            </span>
          ) : (
            t("Not reported by this desktop.")
          )}
          {status.eventsSeen !== undefined && (
            <span className="mt-0.5 block text-micro text-ink-3">
              {t("Buttons seen: {count}", { count: String(status.eventsSeen) })}
            </span>
          )}
        </DiagnosticFact>
        <DiagnosticFact label={t("Window")}>{windowFact(status, windowTest)}</DiagnosticFact>
        <DiagnosticFact label={t("Running game")}>
          {status.game
            ? `${status.game.title}${status.game.pid ? ` · PID ${status.game.pid}` : ""}`
            : t("No game is running.")}
        </DiagnosticFact>
        <DiagnosticFact label={t("Last trigger")}>
          {lastTriggerLabel(status.lastTrigger)}
        </DiagnosticFact>
      </div>

      <p className="mt-3 max-w-[72ch] text-meta leading-relaxed text-ink-3">
        {t("A game in exclusive fullscreen blocks every panel from outside the game, including this overlay. Set the game to borderless or windowed fullscreen and the overlay will appear above it.")}
      </p>

      <div className="mt-4 flex flex-wrap items-center gap-3">
        <Button size="sm" variant="default" icon={<MonitorPlay className="size-3.5" />} onClick={onOpenNow}>
          {t("Open the overlay now")}
        </Button>
        <span className="max-w-[52ch] text-meta leading-relaxed text-ink-3">
          {t("{shortcut} always works while Ludex is focused.", { shortcut: status.keyboardShortcut })}
        </span>
      </div>
    </div>
  );
}

/** One sentence for the window fact: the test result wins, else the last show. */
function windowFact(status: OverlayStatus, test: OverlayWindowResult | null): ReactNode {
  if (test) {
    const outcome = overlayWindowOutcome(test);
    const monitor = test.monitor ? (
      <span className="text-ink-3"> · {t("Monitor: {monitor}", { monitor: test.monitor })}</span>
    ) : null;
    if (outcome === "shown") {
      return (
        <span className="text-ok">
          {t("Floating window created and visible.")}
          {monitor}
        </span>
      );
    }
    if (outcome === "hidden" || outcome === "failed") {
      const error = test.windowError ?? t("unknown reason");
      return (
        <span className="text-danger">
          {outcome === "hidden"
            ? t("Floating window created but not shown: {error}", { error })
            : t("Floating window not created: {error}", { error })}
          {monitor}
        </span>
      );
    }
    return t("Not reported by this desktop.");
  }
  if (status.windowShownAt) {
    return `${t("Window shown")} · ${formatEventTime(status.windowShownAt)}`;
  }
  return t("No window test yet.");
}

/** Local clock for a diagnostic timestamp; a bad date never prints `Invalid Date`. */
function formatEventTime(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleTimeString();
}

function DiagnosticFact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div>
      <p className="text-micro uppercase tracking-[0.12em] text-ink-3">{label}</p>
      <p className="mt-0.5 text-label text-ink-2">{children}</p>
    </div>
  );
}

function checkDot(check: OverlayCheck): string {
  switch (check.level) {
    case "ok":
      return "bg-ok";
    case "warning":
      return "bg-signal";
    case "blocking":
      return "bg-danger";
    default:
      return "bg-ink-3";
  }
}

/** Translate a stable check id; unknown ids fall back to the backend copy. */
function overlayCheckLabel(check: OverlayCheck): string {
  switch (check.id) {
    case "overlay-enabled":
      return t("The overlay is enabled.");
    case "overlay-disabled":
      return t("The overlay is turned off in Settings → Profiles.");
    case "game-enabled":
      return t("The overlay is enabled for this game.");
    case "game-disabled":
      return t("The overlay is turned off for this game.");
    case "listener-running":
      return t("The controller listener is running.");
    case "listener-failed":
    case "listener-stopped":
    case "trigger-unavailable":
      return t("The controller listener is not running.");
    case "listener-unsupported":
      return t("Controller triggers need Windows or Linux. Use the keyboard shortcut.");
    case "controller-connected":
      return t("A controller is connected.");
    case "no-controller":
      return t("No controller detected. Connect a pad and press a button so the system registers it; the keyboard shortcut still works.");
    case "trigger-ready":
      return t("The configured controller gesture is ready.");
    case "trigger-keyboard":
      return t("Only the keyboard shortcut is armed; no controller gesture opens the overlay.");
    case "guide-unsupported-windows":
      return t("Windows does not deliver the Guide/Home button to apps. LB+RB+Start still opens the overlay, or switch the gesture to LB+RB+Start.");
    case "trigger-no-controller":
      return t("The configured gesture needs a connected controller.");
    case "no-game":
      return t("No game is running; open the overlay from here or launch a game first.");
    case "game-windowed":
      return t("The game window is windowed or borderless: the overlay can draw above it.");
    case "game-fullscreen":
    case "game-fullscreen-unknown":
      return t("The game looks fullscreen. Exclusive fullscreen hides every panel drawn from outside the game; borderless or windowed fullscreen lets the overlay appear above it.");
    case "floating-window":
      return t("A floating always-on-top window can be drawn above a borderless game.");
    case "no-floating-window":
      return t("This platform can only draw the overlay inside Ludex, not over a game.");
    default:
      return check.message;
  }
}

function effectiveGestureLabel(status: OverlayStatus): string {
  switch (status.triggerStatus.effective) {
    case "guide":
      return t("Double Home (Guide button)");
    case "chord":
      return t("LB+RB+Start");
    case "keyboard":
      return t("Keyboard only");
    default:
      return t("No controller gesture available");
  }
}

function lastTriggerLabel(event: OverlayTriggerEvent | null): string {
  if (!event) return t("No trigger event yet.");
  const time = formatEventTime(event.at);
  switch (event.outcome) {
    case "openedWindow":
      return `${t("Opened in the floating window.")} · ${time}`;
    case "openedInApp":
      return `${t("Opened inside Ludex.")} · ${time}`;
    case "closed":
      return `${t("Closed.")} · ${time}`;
    case "ignored":
      return `${t("Ignored: {reason}", { reason: event.detail ?? t("unknown reason") })} · ${time}`;
    default:
      return time;
  }
}
