import { useEffect, useMemo, useState } from "react";
import { motion, useReducedMotion } from "motion/react";
import { cn } from "@/lib/cn";
import { formatPercent } from "@/lib/format";
import { t, useI18n } from "@/lib/i18n";
import type { User } from "@/lib/types";
import { useActiveCount, useQueueSummary } from "@/state/downloads";
import { Monogram } from "@/components/brand";
import { UserAvatar } from "@/components/community/user-avatar";
import { reveal } from "./entrance";
import { Glyph } from "./glyphs";
import { BOOT_DELAY, BOOT_DURATION } from "./motion";

/* --------------------------------------------------------------------------
   The console bar (#196, #268).

   Status only, never a focus stop: at three metres a clock is useful and a
   dashboard is not. The bar carries the monogram, not the wordmark — a 17px
   wordmark is a grey smear at three metres — and every word is sentence case.
   Mono is reserved for the figures: clock, counts, percentages, battery.

   No dead glyphs: Power moved to the band, so the bar no longer ends on a
   control that does nothing.
   -------------------------------------------------------------------------- */

export interface SystemBarProps {
  user: User | null;
  serverLabel: string | null;
  online: boolean;
  padConnected: boolean;
  padFamily: string;
  padBattery: number | null;
  padCharging: boolean;
  downloads?: number;
  downloadsPercent?: number | null;
  refreshing: boolean;
  consoleActive: boolean;
  unreadNotifications?: number;
  onOpenNotifications?: () => void;
}

/** The clock lands on the next minute boundary instead of drifting by 30 s. */
function useMinuteClock(intl: string): { label: string; iso: string } {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    let timer: number | undefined;
    const schedule = () => {
      const untilNextMinute = 60_000 - (Date.now() % 60_000);
      timer = window.setTimeout(() => {
        setNow(new Date());
        schedule();
      }, untilNextMinute + 40);
    };
    schedule();
    return () => {
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, []);
  return useMemo(
    () => ({
      label: new Intl.DateTimeFormat(intl, { hour: "2-digit", minute: "2-digit" }).format(now),
      iso: now.toISOString(),
    }),
    [intl, now],
  );
}

function Slot({
  children,
  className,
  title,
}: {
  children: React.ReactNode;
  className?: string;
  title?: string;
}) {
  return (
    <div
      title={title}
      className={cn(
        "flex min-w-0 shrink-0 items-center gap-2 px-3 text-bp-meta text-ink-3",
        className,
      )}
    >
      {children}
    </div>
  );
}

export function SystemBar({
  user,
  serverLabel,
  online,
  padConnected,
  padFamily,
  padBattery,
  padCharging,
  downloads: downloadsProp,
  downloadsPercent: downloadsPercentProp,
  refreshing,
  consoleActive,
  unreadNotifications = 0,
  onOpenNotifications,
}: SystemBarProps) {
  const { meta } = useI18n();
  const clock = useMinuteClock(meta.intl);
  const name = user?.username ?? null;
  const reduceMotion = useReducedMotion();
  // #267: the bar subscribes to the two numbers it prints instead of receiving
  // them from the shell. A progress tick repaints this header only, never the
  // rail, the stage or the overlays.
  const liveCount = useActiveCount();
  const liveSummary = useQueueSummary();
  const downloads = downloadsProp ?? liveCount;
  const downloadsPercent = downloadsPercentProp ?? liveSummary.percent;

  return (
    <motion.header
      aria-label={t("Status")}
      data-bp-bleed-top="true"
      className="relative z-20 flex shrink-0 items-stretch overflow-hidden border-b bp-chrome bp-pad-x"
      {...reveal(reduceMotion, { y: 6, delay: BOOT_DELAY.bar, duration: BOOT_DURATION })}
    >
      <div className="flex min-w-0 shrink items-center gap-3">
        <Monogram size={28} className="h-[calc(var(--bp-u)*28)] w-auto text-ink" />
        <span className="text-bp-body font-semibold tracking-[-0.02em] text-ink">Ludex</span>
        {refreshing && (
          <span role="status" className="flex items-center gap-1.5 text-bp-meta text-ink-3">
            <span className="bp-dot animate-pulse motion-reduce:animate-none" aria-hidden="true" />
            <span className="truncate">{t("Refreshing")}</span>
          </span>
        )}
      </div>

      <div className="ms-auto flex min-w-0 shrink-0 items-stretch divide-x divide-line">
        {name && (
          <Slot className="max-w-[30ch]" title={t("Signed in as {name}", { name })}>
            {onOpenNotifications ? (
              <button
                type="button"
                tabIndex={-1}
                onClick={onOpenNotifications}
                aria-label={
                  unreadNotifications > 0
                    ? t("Notifications ({count} unread)", { count: unreadNotifications })
                    : t("Notifications")
                }
                className="flex min-w-0 items-center gap-2 text-start text-ink-2 transition-colors hover:text-ink"
              >
                <span className="relative shrink-0">
                  <UserAvatar name={name} src={user?.avatarUrl} size={22} />
                  {unreadNotifications > 0 && (
                    <span
                      className="bp-dot absolute -end-1 -top-1"
                      aria-hidden="true"
                    />
                  )}
                </span>
                <span className="hidden min-w-0 flex-col justify-center leading-tight sm:flex">
                  <span className="truncate text-bp-meta text-ink-2">@{user?.username}</span>
                  {serverLabel && (
                    <span className="hidden max-w-[16ch] truncate text-bp-meta text-ink-3 lg:block">
                      {serverLabel}
                    </span>
                  )}
                </span>
              </button>
            ) : (
              <>
                <UserAvatar name={name} src={user?.avatarUrl} size={22} />
                <span className="hidden min-w-0 flex-col justify-center leading-tight sm:flex">
                  <span className="truncate text-bp-meta text-ink-2">@{user?.username}</span>
                  {serverLabel && (
                    <span className="hidden max-w-[16ch] truncate text-bp-meta text-ink-3 lg:block">
                      {serverLabel}
                    </span>
                  )}
                </span>
              </>
            )}
          </Slot>
        )}

        <Slot className={cn("hidden sm:flex", !online && "text-warn")}>
          <span
            className={cn("bp-dot", !online && "bg-warn")}
            data-tone={online ? undefined : "muted"}
            aria-hidden="true"
          />
          <span className="truncate">{online ? t("Online") : t("Offline")}</span>
        </Slot>

        {downloads > 0 && (
          <Slot className="hidden sm:flex" title={t("{count} downloading", { count: downloads })}>
            <Glyph name="downloads" className="size-[calc(var(--bp-u)*20)] text-signal-text" />
            <span className="num text-ink-2">{downloads}</span>
            {downloadsPercent !== null && (
              <span className="num min-w-[4ch] text-end text-ink-3">
                {formatPercent(downloadsPercent)}
              </span>
            )}
          </Slot>
        )}

        {padConnected && (
          <Slot className="hidden md:flex" title={padFamily}>
            <span className="bp-dot" aria-hidden="true" />
            <span className="max-w-[12ch] truncate text-ink-2">{padFamily}</span>
            {padBattery !== null && (
              <span className="flex items-center gap-1">
                <span className="num text-ink-2">{padBattery}%</span>
                {padCharging && <Glyph name="check" className="size-[calc(var(--bp-u)*18)]" />}
              </span>
            )}
          </Slot>
        )}

        {consoleActive && (
          <Slot className="hidden lg:flex" title={t("Console mode")}>
            <span className="truncate">{t("Console mode")}</span>
          </Slot>
        )}

        <Slot className="min-w-[7ch] justify-end">
          <time dateTime={clock.iso} aria-live="off" className="num text-bp-meta text-ink-2">
            {clock.label}
          </time>
        </Slot>
      </div>
    </motion.header>
  );
}
