import { useEffect, useMemo, useState } from "react";
import { motion, useReducedMotion } from "motion/react";
import {
  BatteryCharging,
  BatteryFull,
  BatteryLow,
  BatteryMedium,
  Download,
  Gamepad2,
  Power,
  RefreshCw,
  Wifi,
  WifiOff,
} from "lucide-react";
import { cn } from "@/lib/cn";
import { formatPercent } from "@/lib/format";
import { t, useI18n } from "@/lib/i18n";
import type { User } from "@/lib/types";
import { Wordmark } from "@/components/brand";
import { UserAvatar } from "@/components/community/user-avatar";
import { reveal } from "./entrance";
import { BOOT_DELAY, BOOT_DURATION } from "./motion";

/* --------------------------------------------------------------------------
   The console system bar (#196).

   Status only, never a focus stop: at three metres a camera is useful and a
   dashboard is not. Every figure uses tabular numerals and a fixed slot, so a
   percentage ticking to 100 % never reflows the bar under the player's eyes.
   -------------------------------------------------------------------------- */

export interface SystemBarProps {
  user: User | null;
  serverLabel: string | null;
  online: boolean;
  padConnected: boolean;
  padFamily: string;
  padBattery: number | null;
  padCharging: boolean;
  downloads: number;
  downloadsPercent: number | null;
  refreshing: boolean;
  consoleActive: boolean;
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

function BatteryGlyph({ level, charging }: { level: number; charging: boolean }) {
  const Icon = charging ? BatteryCharging : level >= 70 ? BatteryFull : level >= 35 ? BatteryMedium : BatteryLow;
  return (
    <span className="flex items-center gap-1">
      <Icon className="size-4" aria-hidden="true" />
      <span className="num text-[11px] text-ink-2">{level}%</span>
    </span>
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
      className={cn("flex min-w-0 shrink-0 items-center gap-2 px-3 font-mono text-[10px] uppercase tracking-[0.12em] text-ink-3", className)}
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
  downloads,
  downloadsPercent,
  refreshing,
  consoleActive,
}: SystemBarProps) {
  const { meta } = useI18n();
  const clock = useMinuteClock(meta.intl);
  const name = user?.username ?? null;
  const reduceMotion = useReducedMotion();

  return (
    <motion.header
      aria-label={t("System")}
      className="relative z-20 flex h-[var(--bp-bar-h)] shrink-0 items-stretch border-b border-line bg-bg-deep bp-pad-x"
      {...reveal(reduceMotion, { y: 6, delay: BOOT_DELAY.bar, duration: BOOT_DURATION })}
    >
      <div className="flex min-w-0 items-center gap-3">
        <Wordmark height={17} />
        <span className="font-mono text-[10px] uppercase tracking-[0.18em] text-ink-3">
          {t("Big Picture")}
        </span>
        {refreshing && (
          <span role="status" className="flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.14em] text-ink-3">
            <RefreshCw className="size-3 animate-spin [animation-duration:1.8s] motion-reduce:animate-none" aria-hidden="true" />
            <span className="sr-only">{t("Refreshing")}</span>
          </span>
        )}
        {consoleActive && (
          <span className="hidden items-center gap-1.5 border border-line px-1.5 py-0.5 font-mono text-[9px] uppercase tracking-[0.14em] text-signal-text sm:flex">
            {t("console")}
          </span>
        )}
      </div>

      <div className="ms-auto flex min-w-0 items-stretch divide-x divide-line">
        {name && (
          <Slot className="max-w-[30ch]" title={t("Signed in as {name}", { name })}>
            <UserAvatar name={name} src={user?.avatarUrl} size={22} />
            <span className="flex min-w-0 flex-col justify-center leading-tight">
              <span className="truncate text-[11px] normal-case tracking-normal text-ink-2">@{user?.username}</span>
              {serverLabel && (
                <span className="hidden max-w-[16ch] truncate text-[9px] tracking-[0.08em] text-ink-3 lg:block">
                  {serverLabel}
                </span>
              )}
            </span>
          </Slot>
        )}

        <Slot title={undefined} className={online ? undefined : "text-ink-2"}>
          {online ? (
            <Wifi className="size-3.5 text-ok" aria-hidden="true" />
          ) : (
            <WifiOff className="size-3.5 text-warn" aria-hidden="true" />
          )}
          <span className={online ? "text-ink-2" : "text-warn"}>{online ? t("Online") : t("Offline")}</span>
        </Slot>

        {downloads > 0 && (
          <Slot className="hidden sm:flex" title={t("{count} downloading", { count: downloads })}>
            <Download className="size-3.5 text-signal-text" aria-hidden="true" />
            <span className="num text-ink-2">{downloads}</span>
            {downloadsPercent !== null && (
              <span className="num min-w-[4ch] text-end text-ink-3">{formatPercent(downloadsPercent)}</span>
            )}
          </Slot>
        )}

        {padConnected && (
          <Slot className="hidden md:flex" title={padFamily}>
            <Gamepad2 className="size-3.5 text-signal-text" aria-hidden="true" />
            <span className="max-w-[12ch] truncate normal-case tracking-normal text-ink-2">{padFamily}</span>
            {padBattery !== null && <BatteryGlyph level={padBattery} charging={padCharging} />}
          </Slot>
        )}

        <Slot className="min-w-[7ch] justify-end">
          <time dateTime={clock.iso} aria-live="off" className="num text-[12px] text-ink-2">
            {clock.label}
          </time>
        </Slot>

        <Slot className="pe-0">
          <Power className="size-3.5 text-signal-text" aria-hidden="true" />
        </Slot>
      </div>
    </motion.header>
  );
}
