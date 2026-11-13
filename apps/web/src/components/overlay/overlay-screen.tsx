import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Gamepad2, Library, Play, Square, Trophy, X } from "lucide-react";
import { api, tokenStore } from "@/lib/api";
import { formatBytes, formatPercent, formatSpeed } from "@/lib/format";
import { errorMessage } from "@/lib/http";
import { isNative, native } from "@/lib/native";
import { useGamepad } from "@/lib/gamepad";
import { applyGamepadFocus, moveTargetIndex, type MoveAction } from "@/lib/gamepad-core";
import { useControllerProfile } from "@/lib/use-controller-profile";
import { jobProgressPercent } from "@/lib/install-phase";
import {
  defaultOverlayTab,
  formatOverlayClock,
  isOverlayCloseKey,
  normalizeOverlayStatus,
  overlayDurationParts,
  overlayElapsedSeconds,
  switchOverlayTab,
  trapFocusIndex,
  visibleOverlayTabs,
} from "@/lib/overlay";
import type { InstallJob, InstalledGame, OverlayStatus, OverlayTabId } from "@/lib/types";
import { t } from "@/lib/i18n";

/**
 * The in-game overlay (#144), rendered in two places from the same component:
 *
 * - the floating `overlay` window, above a borderless-fullscreen game, opened
 *   by the native controller listener or `open_overlay`;
 * - inside the main window, opened by Shift+Tab while Ludex is focused (phase
 *   1, and the fallback when a floating window is unavailable).
 *
 * It is a focus trap with gamepad-first navigation, but never a modal to the
 * gamepad hook: `aria-modal` would make the library's `useGamepad` swallow every
 * button as "dialog open". Closing is B/Escape (or Home again through the
 * native listener); the game process is never touched and downloads keep
 * running in the Rust install queue.
 */

const ACTIVE_PHASES = new Set([
  "queued",
  "waitingForSpace",
  "downloading",
  "verifying",
  "extracting",
  "finalizing",
]);

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

function jobPercent(job: InstallJob): number | null {
  // #223: an indeterminate bar while the archive is verified, never a frozen
  // 100 % with the download's last rate.
  return jobProgressPercent(job);
}

export interface OverlayScreenProps {
  mode: "window" | "in-app";
  onClose: () => void;
}

export function OverlayScreen({ mode, onClose }: OverlayScreenProps) {
  const queryClient = useQueryClient();
  const rootRef = useRef<HTMLDivElement>(null);
  const profile = useControllerProfile();
  const [tab, setTab] = useState<OverlayTabId>("game");
  const [now, setNow] = useState(() => Date.now());
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirmExit, setConfirmExit] = useState(false);

  const statusQuery = useQuery({
    queryKey: ["overlay-status"],
    queryFn: () => native.overlayStatus(),
    enabled: isNative(),
    refetchInterval: 2_000,
    staleTime: 0,
  });
  const status = useMemo<OverlayStatus>(
    () => normalizeOverlayStatus(statusQuery.data ?? null),
    [statusQuery.data],
  );

  const game = status.game;
  const tabs = useMemo(() => visibleOverlayTabs(status.tabs), [status.tabs]);

  // Live settings: opacity/scale/tabs apply while the overlay is open.
  useEffect(() => {
    if (!isNative()) return;
    let disposed = false;
    let off: (() => void) | undefined;
    void native.onOverlaySettings((status) => {
      // Immediate apply (opacity/scale/tabs); the poll stays as a safety net.
      queryClient.setQueryData(["overlay-status"], status);
    }).then((unlisten) => {
      if (disposed) unlisten();
      else off = unlisten;
    });
    return () => {
      disposed = true;
      off?.();
    };
  }, [queryClient]);

  // Session clock. The backend reports elapsedSeconds on every poll; ticking
  // locally keeps the display smooth without a one-second IPC round trip.
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1_000);
    return () => clearInterval(timer);
  }, []);

  useEffect(() => {
    if (!tabs.includes(tab)) setTab(defaultOverlayTab(status));
  }, [status, tab, tabs]);

  useEffect(() => {
    setConfirmExit(false);
    // Focus the first target of the new tab so a controller player is never
    // stranded outside the panel.
    const frame = requestAnimationFrame(() => focusTargets(rootRef.current)?.[0]?.focus({ preventScroll: true }));
    return () => cancelAnimationFrame(frame);
  }, [tab]);

  useEffect(() => {
    const frame = requestAnimationFrame(() => focusTargets(rootRef.current)?.[0]?.focus({ preventScroll: true }));
    return () => cancelAnimationFrame(frame);
  }, []);

  const close = useCallback(() => {
    if (isNative()) void native.closeOverlay().catch(() => undefined);
    onClose();
  }, [onClose]);

  const move = useCallback((action: MoveAction) => {
    const targets = focusTargets(rootRef.current);
    if (targets.length === 0) return;
    const rects = targets.map((target) => target.getBoundingClientRect());
    const current = Math.max(0, targets.indexOf(document.activeElement as HTMLElement));
    applyGamepadFocus(targets, moveTargetIndex(rects, current, action), { scroll: true });
  }, []);

  const selectTab = useCallback((next: OverlayTabId) => {
    setTab(next);
  }, []);

  useGamepad(
    (action) => {
      switch (action) {
        case "back":
          close();
          return;
        case "prevTab":
          selectTab(switchOverlayTab(tabs, tab, -1));
          return;
        case "nextTab":
          selectTab(switchOverlayTab(tabs, tab, 1));
          return;
        case "confirm": {
          const focused = document.activeElement;
          if (focused instanceof HTMLElement && focused.dataset.overlayTarget !== undefined) {
            focused.click();
          }
          return;
        }
        case "left":
        case "right":
        case "up":
        case "down":
          move(action);
          return;
        default:
          return;
      }
    },
    { enabled: true, profile },
  );

  const onKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (isOverlayCloseKey(event.key)) {
      event.preventDefault();
      close();
      return;
    }
    if (event.key === "Tab") {
      const targets = focusTargets(rootRef.current);
      if (targets.length === 0) return;
      event.preventDefault();
      const current = Math.max(0, targets.indexOf(document.activeElement as HTMLElement));
      applyGamepadFocus(targets, trapFocusIndex(current, targets.length, event.shiftKey), {
        scroll: true,
      });
      return;
    }
    const direction =
      event.key === "ArrowLeft"
        ? "left"
        : event.key === "ArrowRight"
          ? "right"
          : event.key === "ArrowUp"
            ? "up"
            : event.key === "ArrowDown"
              ? "down"
              : null;
    if (direction) {
      // Same spatial movement as the d-pad, so mouse-less keyboard play works.
      event.preventDefault();
      move(direction);
    }
  };

  const exitGame = async () => {
    if (!game) return;
    if (!confirmExit) {
      setConfirmExit(true);
      return;
    }
    setBusy("exit");
    setError(null);
    try {
      await native.stopGame(game.sessionId);
      close();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(null);
    }
  };

  const disableForGame = async () => {
    if (!game) return;
    setBusy("disable");
    try {
      await native.setOverlayGameDisabled(game.gameId, true);
      close();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(null);
    }
  };

  const bigPicture = async () => {
    setBusy("bigpicture");
    try {
      await native.overlayShowBigPicture();
      onClose();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(null);
    }
  };

  const scale = status.scale;
  const elapsed = game ? overlayElapsedSeconds(game.startedAt, now) : 0;
  const total = overlayDurationParts(game?.totalSeconds ?? 0);

  return (
    <div
      ref={rootRef}
      role="dialog"
      aria-label={t("In-game overlay")}
      onKeyDown={onKeyDown}
      className="fixed inset-0 z-[200] flex items-center justify-center p-3 sm:p-6"
      data-overlay-mode={mode}
    >
      <div className="absolute inset-0 bg-bg-deep/70 backdrop-blur-[3px]" aria-hidden="true" />
      <div
        className="relative flex max-h-[94vh] w-full flex-col overflow-hidden rounded-[3px] border border-line-2 shadow-2xl"
        style={{ maxWidth: `${Math.round(1120 * scale)}px`, fontSize: `${Math.round(16 * scale)}px` }}
      >
        <div
          aria-hidden="true"
          className="absolute inset-0 bg-bg-deep"
          style={{ opacity: status.opacity }}
        />
        <div className="relative flex min-h-0 flex-1 flex-col">
          <header className="flex flex-wrap items-center justify-between gap-3 border-b border-line px-5 py-3.5">
            <div className="flex items-baseline gap-3">
              <span className="font-mono text-[10px] uppercase tracking-[0.2em] text-signal-text">
                {t("Ludex overlay")}
              </span>
              {game ? (
                <span className="max-w-[52ch] truncate text-[13.5px] text-ink">{game.title}</span>
              ) : (
                <span className="text-[13.5px] text-ink-3">{t("No game is running")}</span>
              )}
            </div>
            <div className="flex items-center gap-4">
              <span className="font-mono text-[12px] tabular-nums text-ink-2">{formatOverlayClock(elapsed)}</span>
              <button
                type="button"
                data-overlay-target
                onClick={close}
                aria-label={t("Close the overlay")}
                className="grid size-8 place-items-center rounded-[2px] border border-line text-ink-2 hover:border-line-2 hover:text-ink"
              >
                <X size={15} />
              </button>
            </div>
          </header>

          <nav
            aria-label={t("Overlay sections")}
            className="flex flex-wrap items-center gap-1 border-b border-line px-3 py-2"
          >
            {tabs.map((entry) => (
              <button
                key={entry}
                type="button"
                data-overlay-target
                aria-pressed={entry === tab}
                onClick={() => selectTab(entry)}
                className={
                  entry === tab
                    ? "rounded-[2px] bg-surface-2 px-3 py-1.5 font-mono text-[10.5px] uppercase tracking-[0.14em] text-ink"
                    : "rounded-[2px] px-3 py-1.5 font-mono text-[10.5px] uppercase tracking-[0.14em] text-ink-3 hover:text-ink-2"
                }
              >
                {overlayTabLabel(entry)}
              </button>
            ))}
          </nav>

          <div className="min-h-0 flex-1 overflow-y-auto px-5 py-5">
            {error && (
              <p className="mb-4 rounded-[2px] border border-danger/40 bg-danger/10 px-3 py-2 text-[12.5px] text-danger">
                {error}
              </p>
            )}
            {tab === "game" && (
              <GameTab
                status={status}
                elapsed={elapsed}
                total={total}
                busy={busy}
                confirmExit={confirmExit}
                onResume={close}
                onExit={() => void exitGame()}
                onDisable={() => void disableForGame()}
                onBigPicture={() => void bigPicture()}
              />
            )}
            {tab === "library" && <LibraryTab runningGameId={game?.gameId ?? null} onLaunched={close} />}
            {tab === "downloads" && <DownloadsTab />}
            {tab === "achievements" && <AchievementsTab gameId={game?.gameId ?? null} />}
          </div>

          <footer className="flex flex-wrap items-center justify-between gap-3 border-t border-line px-5 py-3">
            <ControllerStatus status={status} />
            <div className="flex flex-wrap items-center gap-x-4 gap-y-1 font-mono text-[10px] uppercase tracking-[0.12em] text-ink-3">
              <span>{t("Esc or B closes")}</span>
              <span>{t("LB / RB switch tabs")}</span>
              <span>{status.keyboardShortcut}</span>
            </div>
          </footer>
        </div>
      </div>
    </div>
  );
}

/** Visible, enabled overlay targets in DOM order. */
function focusTargets(root: HTMLElement | null): HTMLElement[] {
  if (!root) return [];
  return Array.from(root.querySelectorAll<HTMLElement>("[data-overlay-target]")).filter(
    (element) => element.offsetParent !== null && !("disabled" in element && element.disabled),
  );
}

function GameTab({
  status,
  elapsed,
  total,
  busy,
  confirmExit,
  onResume,
  onExit,
  onDisable,
  onBigPicture,
}: {
  status: OverlayStatus;
  elapsed: number;
  total: { hours: number; minutes: number };
  busy: string | null;
  confirmExit: boolean;
  onResume: () => void;
  onExit: () => void;
  onDisable: () => void;
  onBigPicture: () => void;
}) {
  const game = status.game;
  if (!game) {
    return (
      <div className="flex min-h-[240px] flex-col items-start justify-center gap-4">
        <p className="max-w-[52ch] text-[15px] leading-relaxed text-ink-2">
          {t("Start a game from Ludex, or open Big Picture to pick one from the couch.")}
        </p>
        <OverlayButton onClick={onBigPicture} variant="primary">
          <Play size={14} />
          {t("Open Big Picture")}
        </OverlayButton>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-6">
      <div>
        <p className="font-mono text-[10px] uppercase tracking-[0.18em] text-signal-text">
          {t("Now playing")}
        </p>
        <h2 className="mt-2 max-w-[60ch] text-[clamp(22px,3vw,34px)] font-semibold leading-tight tracking-[-0.02em] text-ink">
          {game.title}
        </h2>
        <div className="mt-3 flex flex-wrap items-center gap-x-6 gap-y-1 font-mono text-[11px] text-ink-3">
          <span>
            {t("This session")} {formatOverlayClock(elapsed)}
          </span>
          <span>
            {t("Total playtime")}{" "}
            {total.hours > 0
              ? t("{hours} h {minutes} min", { hours: String(total.hours), minutes: String(total.minutes) })
              : t("{minutes} min", { minutes: String(total.minutes) })}
          </span>
          <span className="text-ink-3">{t("The game keeps running while the overlay is open.")}</span>
        </div>
      </div>

      <div className="flex flex-wrap gap-2">
        <OverlayButton onClick={onResume} variant="primary">
          <Play size={14} />
          {t("Resume")}
        </OverlayButton>
        <OverlayButton onClick={onExit} variant="danger" disabled={busy !== null}>
          <Square size={13} />
          {confirmExit ? t("Confirm exit") : t("Exit game")}
        </OverlayButton>
        <OverlayButton onClick={onBigPicture} disabled={busy !== null}>
          <Library size={14} />
          {t("Big Picture")}
        </OverlayButton>
        <OverlayButton onClick={onDisable} disabled={busy !== null}>
          <Gamepad2 size={14} />
          {t("Disable overlay for this game")}
        </OverlayButton>
      </div>

      <p className="max-w-[70ch] text-[12px] leading-relaxed text-ink-3">{status.platformNote}</p>
    </div>
  );
}

function LibraryTab({
  runningGameId,
  onLaunched,
}: {
  runningGameId: number | null;
  onLaunched: () => void;
}) {
  const [busy, setBusy] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const installed = useQuery({
    queryKey: ["overlay-installed"],
    queryFn: () => native.listInstalled(),
    enabled: isNative(),
    staleTime: 30_000,
  });
  const games: InstalledGame[] = installed.data ?? [];

  const launch = async (game: InstalledGame) => {
    setBusy(game.gameId);
    setError(null);
    try {
      await native.launchGame({ gameId: game.gameId, executable: game.executable });
      onLaunched();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(null);
    }
  };

  if (installed.isPending) return <OverlayEmpty>{t("Reading the installed games…")}</OverlayEmpty>;
  if (games.length === 0) return <OverlayEmpty>{t("No installed game on this device yet.")}</OverlayEmpty>;

  return (
    <div className="flex flex-col gap-2">
      {error && <p className="text-[12.5px] text-danger">{error}</p>}
      {games.slice(0, 40).map((game) => {
        const running = game.gameId === runningGameId;
        return (
          <button
            key={game.gameId}
            type="button"
            data-overlay-target
            disabled={busy !== null || running}
            onClick={() => void launch(game)}
            className="flex items-center justify-between gap-4 border-b border-line px-1 py-2.5 text-left hover:bg-surface-2/60 disabled:opacity-55"
          >
            <span className="min-w-0">
              <span className="block truncate text-[13.5px] text-ink">{game.title}</span>
              <span className="mt-0.5 block font-mono text-[10.5px] text-ink-3">
                {running ? t("Running") : (game.version ?? t("Installed"))}
                {game.sizeBytes > 0 ? ` · ${formatBytes(game.sizeBytes)}` : ""}
              </span>
            </span>
            <Play size={14} className="shrink-0 text-signal-text" />
          </button>
        );
      })}
    </div>
  );
}

function DownloadsTab() {
  const jobs = useQuery({
    queryKey: ["overlay-jobs"],
    queryFn: () => native.listInstallJobs(),
    enabled: isNative(),
    refetchInterval: 2_500,
  });
  const active = (jobs.data ?? []).filter((job) => ACTIVE_PHASES.has(job.phase));

  if (jobs.isPending) return <OverlayEmpty>{t("Reading the download queue…")}</OverlayEmpty>;
  if (active.length === 0) {
    return <OverlayEmpty>{t("No active download. Installs keep running when the overlay closes.")}</OverlayEmpty>;
  }

  return (
    <div className="flex flex-col gap-4">
      {active.map((job) => {
        const percent = jobPercent(job);
        return (
          <div key={job.jobId} className="border-b border-line pb-3">
            <div className="flex items-baseline justify-between gap-4">
              <span className="font-mono text-[11px] text-ink">{job.phase}</span>
              <span className="font-mono text-[11px] tabular-nums text-ink-3">
                {percent === null ? t("estimating") : formatPercent(percent)}
                {job.reconnecting
                  ? ` · ${t("Connection lost, resuming from {percent}", { percent: formatPercent(percent ?? 0) })}`
                  : job.speedBps > 0
                    ? ` · ${formatSpeed(job.speedBps)}`
                    : ""}
              </span>
            </div>
            <div className="mt-2 h-1 overflow-hidden rounded-full bg-line">
              <div
                className="h-full bg-signal transition-[width] duration-500"
                style={{ width: `${percent ?? 33}%` }}
              />
            </div>
            <p className="mt-1.5 truncate font-mono text-[10.5px] text-ink-3">
              {job.bytesDone > 0 ? formatBytes(job.bytesDone) : job.message ?? ""}
              {job.bytesTotal ? ` / ${formatBytes(job.bytesTotal)}` : ""}
            </p>
          </div>
        );
      })}
    </div>
  );
}

function AchievementsTab({ gameId }: { gameId: number | null }) {
  const achievements = useQuery({
    queryKey: ["overlay-achievements", gameId],
    queryFn: () => api.gameAchievements(gameId!),
    enabled: isNative() && Boolean(gameId && gameId > 0) && Boolean(tokenStore.get()),
    staleTime: 60_000,
    retry: false,
  });

  if (!gameId || gameId <= 0) return <OverlayEmpty>{t("Achievements appear when a game is running.")}</OverlayEmpty>;
  if (achievements.isPending) return <OverlayEmpty>{t("Reading achievements…")}</OverlayEmpty>;
  if (achievements.isError) return <OverlayEmpty>{t("Achievements need a reachable server.")}</OverlayEmpty>;

  const data = achievements.data;
  if (!data || data.total === 0) return <OverlayEmpty>{t("No achievement is defined for this game yet.")}</OverlayEmpty>;

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-baseline justify-between gap-3">
        <span className="font-mono text-[11px] uppercase tracking-[0.14em] text-signal-text">
          {t("Completion")}
        </span>
        <span className="font-mono text-[12px] tabular-nums text-ink-2">
          {formatPercent(data.completion)} · {data.unlocked}/{data.total}
        </span>
      </div>
      <div className="h-1 overflow-hidden rounded-full bg-line">
        <div className="h-full bg-signal" style={{ width: `${data.completion}%` }} />
      </div>
      <ul className="flex flex-col gap-2">
        {data.achievements.slice(0, 8).map((achievement) => (
          <li
            key={achievement.id}
            className="flex items-center justify-between gap-4 border-b border-line pb-1.5"
          >
            <span className={achievement.unlocked ? "text-[13px] text-ink" : "text-[13px] text-ink-3"}>
              {achievement.name}
            </span>
            <Trophy
              size={13}
              className={achievement.unlocked ? "shrink-0 text-signal-text" : "shrink-0 text-ink-3"}
            />
          </li>
        ))}
      </ul>
    </div>
  );
}

function ControllerStatus({ status }: { status: OverlayStatus }) {
  const listener = status.listener;
  if (!listener.available) {
    return (
      <span className="flex items-center gap-2 font-mono text-[10px] uppercase tracking-[0.12em] text-ink-3">
        <Gamepad2 size={13} />
        {listener.reason ?? t("No global controller listener")}
      </span>
    );
  }
  if (listener.controllers.length === 0) {
    return (
      <span className="flex items-center gap-2 font-mono text-[10px] uppercase tracking-[0.12em] text-ink-3">
        <Gamepad2 size={13} />
        {t("No controller detected")}
      </span>
    );
  }
  return (
    <span className="flex flex-wrap items-center gap-x-3 gap-y-1 font-mono text-[10px] uppercase tracking-[0.12em] text-ink-3">
      {listener.controllers.map((pad) => (
        <span key={pad.id} className={pad.active ? "flex items-center gap-1.5 text-ink" : "flex items-center gap-1.5"}>
          <Gamepad2 size={13} />
          <span className="max-w-[24ch] truncate">{pad.name}</span>
          {pad.battery !== null && (
            <span className="tabular-nums">
              {pad.battery}%{pad.charging ? "+" : ""}
            </span>
          )}
        </span>
      ))}
    </span>
  );
}

function OverlayEmpty({ children }: { children: React.ReactNode }) {
  return <p className="flex min-h-[200px] max-w-[60ch] items-center text-[13.5px] leading-relaxed text-ink-3">{children}</p>;
}

function OverlayButton({
  children,
  onClick,
  variant = "default",
  disabled,
}: {
  children: React.ReactNode;
  onClick: () => void;
  variant?: "default" | "primary" | "danger";
  disabled?: boolean;
}) {
  const tone =
    variant === "primary"
      ? "border-signal bg-signal text-on-signal hover:bg-signal/90"
      : variant === "danger"
        ? "border-danger/50 text-danger hover:bg-danger/10"
        : "border-line-2 text-ink hover:bg-surface-2";
  return (
    <button
      type="button"
      data-overlay-target
      disabled={disabled}
      onClick={onClick}
      className={`inline-flex h-9 items-center gap-2 rounded-[2px] border px-3.5 text-[12.5px] disabled:opacity-50 ${tone}`}
    >
      {children}
    </button>
  );
}
