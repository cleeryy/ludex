import type { ReactNode } from "react";
import { motion } from "motion/react";
import {
  Download,
  Gamepad2,
  Layers,
  LayoutGrid,
  LogOut,
  Play,
  Power,
  RefreshCw,
  Search,
  Settings,
  type LucideIcon,
} from "lucide-react";
import { cn } from "@/lib/cn";
import { formatPercent } from "@/lib/format";
import { t, tn } from "@/lib/i18n";
import type { Collection, InstallJob } from "@/lib/types";
import { Progress } from "@/components/ui/controls";
import type { HintContext, PadFamily } from "./controller";
import type { BigPictureLayout } from "./layout";
import { MOTION_DURATION, REDUCED_FADE, exitOf, transitionOf } from "./motion";
import { HintStrip } from "./parts";

/* --------------------------------------------------------------------------
   Big Picture overlays (#138).

   One shell for the command menu, the collection index, the download queue and
   the power confirmation: a
   non-modal panel (never `aria-modal`, so the gamepad keeps driving it), a
   highlighted row that is also real DOM focus, and a hint strip for the
   context. Escape/B closes; there is no focus trap because a couch player must
   always be able to back out.
   -------------------------------------------------------------------------- */

export function OverlayShell({
  title,
  subtitle,
  context,
  layout,
  connected,
  family,
  reduceMotion,
  instant = false,
  kind = "overlayOpen",
  onClose,
  wide = false,
  children,
}: {
  title: string;
  subtitle?: string;
  context: HintContext;
  layout: BigPictureLayout;
  connected: boolean;
  family: PadFamily;
  reduceMotion: boolean | null;
  /** A held key opened it: land on the final frame. */
  instant?: boolean;
  /** The power confirmation has its own rhythm and cue. */
  kind?: "overlayOpen" | "powerConfirm";
  onClose: () => void;
  wide?: boolean;
  children: ReactNode;
}) {
  const enter = transitionOf(kind, { reduce: reduceMotion, instant });
  const leave = exitOf(kind, { reduce: reduceMotion, instant });
  const scrim = reduceMotion ? REDUCED_FADE : MOTION_DURATION.fast;
  return (
    <motion.div
      className="fixed inset-0 z-40 flex items-end justify-center bg-scrim/70 p-3 sm:items-center sm:p-6"
      role="presentation"
      onClick={onClose}
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0, pointerEvents: "none", transition: { duration: scrim } }}
      transition={{ duration: scrim }}
    >
      <motion.section
        role="group"
        aria-label={title}
        data-big-picture-overlay={context}
        onClick={(event) => event.stopPropagation()}
        className={cn(
          "flex max-h-[86dvh] w-full flex-col border border-line-2 bg-bg-deep shadow-palette",
          wide ? "max-w-[900px]" : "max-w-[560px]",
        )}
        {...enter}
        {...leave}
      >
        <header className="shrink-0 border-b border-line px-4 py-3 sm:px-5">
          <h2 className="text-[15px] font-semibold text-ink">{title}</h2>
          {subtitle && <p className="mt-1 text-[12px] leading-relaxed text-ink-3">{subtitle}</p>}
        </header>
        <div className="min-h-0 flex-1 overflow-y-auto p-2">{children}</div>
        <footer className="shrink-0 border-t border-line px-4 py-2 text-[10.5px] text-ink-3 sm:px-5">
          <HintStrip context={context} layout={layout} connected={connected} family={family} />
        </footer>
      </motion.section>
    </motion.div>
  );
}

/**
 * One overlay row. The active row carries `data-overlay-active` so the screen
 * can move DOM focus onto it, and `data-gamepad-focus` so the TV-sized ring is
 * visible even though the last input was not a keyboard event.
 */
export function OverlayRow({
  active,
  onChoose,
  onHighlight,
  icon,
  label,
  detail,
  trailing,
  tone,
}: {
  active: boolean;
  onChoose: () => void;
  onHighlight: () => void;
  icon?: ReactNode;
  label: ReactNode;
  detail?: ReactNode;
  trailing?: ReactNode;
  tone?: "danger";
}) {
  return (
    <button
      type="button"
      data-overlay-active={active ? "true" : undefined}
      data-gamepad-focus={active ? "true" : undefined}
      onClick={onChoose}
      onPointerEnter={onHighlight}
      className={cn(
        "flex w-full items-center gap-3 border px-3 py-2.5 text-start transition-colors duration-100",
        active ? "border-signal bg-surface-2" : "border-transparent hover:border-line hover:bg-surface/60",
        tone === "danger" && "text-danger",
      )}
    >
      {icon && <span className={cn("shrink-0", active ? "text-signal-text" : "text-ink-3")}>{icon}</span>}
      <span className="min-w-0 flex-1">
        <span className="block truncate text-[13.5px] text-ink">{label}</span>
        {detail && <span className="mt-0.5 block truncate text-[11px] text-ink-3">{detail}</span>}
      </span>
      {trailing && <span className="shrink-0 text-[11px] text-ink-3">{trailing}</span>}
    </button>
  );
}

/* ------------------------------- command menu ------------------------------ */

export type CommandId = "play" | "game" | "search" | "downloads" | "profiles" | "layout" | "settings" | "exit";

export interface CommandDefinition {
  id: CommandId;
  icon: LucideIcon;
  label: () => string;
  detail?: () => string;
}

/**
 * The Start/Guide menu. Order is the couch order: act on the selected game,
 * find, queue, controller, layout, settings, leave. Every entry has a keyboard
 * parity (`↵`, G, `/`, D, P, L, M, Esc) and the exit entry asks again before
 * leaving.
 */
export const COMMAND_ITEMS: readonly CommandDefinition[] = [
  {
    id: "play",
    icon: Play,
    label: () => t("Play or install"),
  },
  {
    id: "game",
    icon: Gamepad2,
    label: () => t("Game page"),
  },
  {
    id: "search",
    icon: Search,
    label: () => t("Search the library"),
    detail: () => t("Find a game without a keyboard."),
  },
  {
    id: "downloads",
    icon: Download,
    label: () => t("Downloads"),
    detail: () => t("Downloads keep running while this is open."),
  },
  {
    id: "profiles",
    icon: Gamepad2,
    label: () => t("Controller profile"),
    detail: () => t("Switch the pad setup and vibration."),
  },
  {
    id: "layout",
    icon: LayoutGrid,
    label: () => t("Layout"),
    detail: () => t("Change how the library is presented."),
  },
  {
    id: "settings",
    icon: Settings,
    label: () => t("Settings"),
  },
  {
    id: "exit",
    icon: LogOut,
    label: () => t("Power"),
    detail: () => t("Quit"),
  },
];

export function CommandMenu({
  active,
  onChoose,
  onHighlight,
  downloads,
  ...shell
}: {
  active: number;
  onChoose: (index: number) => void;
  onHighlight: (index: number) => void;
  downloads: number;
} & Omit<Parameters<typeof OverlayShell>[0], "children" | "title" | "subtitle">) {
  return (
    <OverlayShell
      title={t("Quick actions")}
      subtitle={t("Everything here works with the controller. B closes this menu.")}
      {...shell}
    >
      <ul className="flex flex-col gap-0.5">
        {COMMAND_ITEMS.map((item, index) => {
          const Icon = item.icon;
          return (
            <li key={item.id}>
              <OverlayRow
                active={index === active}
                onChoose={() => onChoose(index)}
                onHighlight={() => onHighlight(index)}
                icon={<Icon className="size-4" aria-hidden="true" />}
                label={item.label()}
                detail={item.detail?.()}
                tone={item.id === "exit" ? "danger" : undefined}
                trailing={item.id === "downloads" && downloads > 0 ? downloads : undefined}
              />
            </li>
          );
        })}
      </ul>
    </OverlayShell>
  );
}

/* -------------------------------- collections ------------------------------ */

/**
 * Collections without leaving the grid (#196). A read-only shelf index: A
 * opens the collection page, B returns to the console. The list is a system
 * surface, so it follows the same non-modal shell as the queue.
 */
export function CollectionsOverlay({
  active,
  onChoose,
  onHighlight,
  collections,
  loading,
  error,
  onRetry,
  ...shell
}: {
  active: number;
  onChoose: (index: number) => void;
  onHighlight: (index: number) => void;
  collections: readonly Collection[];
  loading: boolean;
  error: string | null;
  onRetry: () => void;
} & Omit<Parameters<typeof OverlayShell>[0], "children" | "title" | "subtitle">) {
  return (
    <OverlayShell
      title={t("Collections")}
      subtitle={t("Your manual collections")}
      {...shell}
    >
      {loading ? (
        <div className="flex flex-col gap-1" aria-busy="true" aria-label={t("Loading…")}>
          {[0, 1, 2].map((index) => (
            <span key={index} className="h-[52px] w-full animate-pulse bg-surface-2" aria-hidden="true" />
          ))}
        </div>
      ) : error ? (
        <div className="flex flex-col gap-2 px-1 py-2">
          <p className="text-[12.5px] leading-relaxed text-ink-3">{error}</p>
          <OverlayRow
            active={active === 0}
            onChoose={onRetry}
            onHighlight={() => onHighlight(0)}
            icon={<RefreshCw className="size-4" aria-hidden="true" />}
            label={t("Try again")}
            detail={t("Unavailable")}
          />
        </div>
      ) : collections.length === 0 ? (
        <p className="px-3 py-6 text-center text-[13px] text-ink-3">{t("No collections yet.")}</p>
      ) : (
        <ul className="flex flex-col gap-0.5">
          {collections.map((collection, index) => (
            <li key={collection.id}>
              <OverlayRow
                active={index === active}
                onChoose={() => onChoose(index)}
                onHighlight={() => onHighlight(index)}
                icon={<Layers className="size-4" aria-hidden="true" />}
                label={collection.name}
                detail={`${tn(collection.gameCount, "1 game", "{count} games")} · ${
                  collection.isPublic ? t("Public") : t("Private")
                }`}
                trailing={collection.owned ? undefined : collection.owner.username}
              />
            </li>
          ))}
        </ul>
      )}
    </OverlayShell>
  );
}

/* ------------------------------ confirmation ------------------------------- */

/**
 * One destructive-action confirmation for the system surfaces (#196). It is
 * deliberately not a modal: the pad keeps driving the row highlight, and the
 * caller starts the highlight on Cancel (index 1) so a reflex A never quits.
 */
export function ConfirmPanel({
  title,
  detail,
  confirmLabel,
  tone,
  active,
  onChoose,
  onHighlight,
  ...shell
}: {
  title: string;
  detail: string;
  confirmLabel: string;
  tone?: "danger";
  active: number;
  onChoose: (index: number) => void;
  onHighlight: (index: number) => void;
} & Omit<Parameters<typeof OverlayShell>[0], "children" | "title" | "subtitle">) {
  return (
    <OverlayShell title={title} subtitle={detail} {...shell} kind="powerConfirm">
      <ul className="flex flex-col gap-0.5">
        <li>
          <OverlayRow
            active={active === 0}
            onChoose={() => onChoose(0)}
            onHighlight={() => onHighlight(0)}
            icon={<Power className="size-4" aria-hidden="true" />}
            label={confirmLabel}
            detail={t("Confirm")}
            tone={tone}
          />
        </li>
        <li>
          <OverlayRow
            active={active === 1}
            onChoose={() => onChoose(1)}
            onHighlight={() => onHighlight(1)}
            label={t("Cancel")}
            detail={t("Back")}
          />
        </li>
      </ul>
    </OverlayShell>
  );
}

/* -------------------------------- downloads -------------------------------- */

const PHASE_COPY: Record<InstallJob["phase"], () => string> = {
  queued: () => t("Queued"),
  waitingForSpace: () => t("Waiting for disk space"),
  downloading: () => t("Downloading"),
  verifying: () => t("Verifying"),
  extracting: () => t("Extracting"),
  finalizing: () => t("Finalizing"),
  paused: () => t("Paused"),
  done: () => t("Done"),
  cancelled: () => t("Cancelled"),
  error: () => t("Error"),
  interrupted: () => t("Interrupted"),
};

function jobPercent(job: InstallJob): number | null {
  if (job.phase === "done") return 100;
  if (!job.bytesTotal) return null;
  return Math.min(100, (job.bytesDone / job.bytesTotal) * 100);
}

/**
 * The queue, without leaving the game. It only reads the download provider, so
 * progress keeps moving behind the panel; A opens the full Downloads screen
 * (and leaves Big Picture) when a job needs pausing or a failed retry.
 */
export function DownloadsOverlay({
  active,
  onChoose,
  onHighlight,
  jobs,
  titleFor,
  ...shell
}: {
  active: number;
  onChoose: (index: number) => void;
  onHighlight: (index: number) => void;
  jobs: readonly InstallJob[];
  titleFor: (job: InstallJob) => string;
} & Omit<Parameters<typeof OverlayShell>[0], "children" | "title" | "subtitle">) {
  return (
    <OverlayShell
      title={t("Downloads")}
      subtitle={t("Downloads keep running while this is open.")}
      {...shell}
    >
      {jobs.length === 0 ? (
        <p className="px-3 py-6 text-center text-[13px] text-ink-3">{t("No downloads running.")}</p>
      ) : (
        <ul className="flex flex-col gap-1">
          {jobs.map((job, index) => {
            const percent = jobPercent(job);
            return (
              <li key={job.jobId}>
                <OverlayRow
                  active={index === active}
                  onChoose={() => onChoose(index)}
                  onHighlight={() => onHighlight(index)}
                  icon={<Download className="size-4" aria-hidden="true" />}
                  label={titleFor(job)}
                  detail={
                    <span className="flex items-center gap-2">
                      <span>{PHASE_COPY[job.phase]()}</span>
                      {percent !== null && <span className="num">{formatPercent(percent)}</span>}
                    </span>
                  }
                  trailing={percent !== null ? `${Math.round(percent)}%` : undefined}
                />
                {percent !== null && (
                  <Progress
                    value={percent}
                    className="mt-1 h-1"
                    label={`${titleFor(job)} ${formatPercent(percent)}`}
                  />
                )}
              </li>
            );
          })}
        </ul>
      )}
      <div className="mt-2 border-t border-line pt-2">
        <OverlayRow
          active={active === jobs.length}
          onChoose={() => onChoose(jobs.length)}
          onHighlight={() => onHighlight(jobs.length)}
          label={t("Open downloads")}
          detail={t("Pause, resume or clear finished installs.")}
        />
      </div>
    </OverlayShell>
  );
}
