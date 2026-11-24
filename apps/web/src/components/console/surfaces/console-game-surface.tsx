import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { cn } from "@/lib/cn";
import { formatBytes, formatMinutes, formatRating, formatYear, stateLabel } from "@/lib/format";
import { t } from "@/lib/i18n";
import { qk } from "@/lib/query-keys";
import type { ControllerProfile, GameDetail, GameSummary, InstalledGame } from "@/lib/types";
import { GameCover } from "@/components/art/cover";
import { Glyph } from "@/components/big-picture/glyphs";
import { useInputScope } from "@/components/console/input/input-provider";
import { wrapIndex } from "@/components/big-picture/system-zones";

export type ConsoleGameTab = "overview" | "saves" | "artwork" | "controller";

const TABS: readonly ConsoleGameTab[] = ["overview", "saves", "artwork", "controller"];

function tabLabel(tab: ConsoleGameTab): string {
  switch (tab) {
    case "overview":
      return t("Overview");
    case "saves":
      return t("Saves");
    case "artwork":
      return t("Artwork");
    case "controller":
      return t("Controller");
  }
}

/**
 * Console-native Game surface (`/console/game/:id`).
 *
 * Replaces any desktop `/game/:id` escape: displays cover, title, telemetry,
 * one primary Play/Install action, one secondary Manage action, and four
 * gamepad-navigable tabs (Overview · Saves · Artwork · Controller). `B` pops
 * back to the Library rail at the exact card index.
 */
export function ConsoleGameSurface({
  game,
  detail,
  installed,
  primaryLabel,
  primaryGlyph,
  onPrimary,
  onOpenProfiles,
  onOpenFolder,
  onBack,
  profile,
  padFamilyName,
  online = true,
}: {
  game: GameSummary;
  detail: GameDetail | undefined;
  installed: InstalledGame | undefined;
  primaryLabel: string;
  primaryGlyph: "play" | "install" | "downloads";
  onPrimary: () => void;
  onOpenProfiles: () => void;
  onOpenFolder?: () => void;
  onBack: () => void;
  profile: ControllerProfile;
  padFamilyName: string;
  online?: boolean;
}) {
  const [tab, setTab] = useState<ConsoleGameTab>("overview");
  const [focusArea, setFocusArea] = useState<"actions" | "tabs">("actions");
  const [actionIndex, setActionIndex] = useState(0);

  const savesQuery = useQuery({
    queryKey: qk.saves.forGame(game.id),
    queryFn: () => api.listSaves(game.id),
    enabled: tab === "saves" && online,
    staleTime: 30_000,
    retry: 0,
  });

  const secondaryActions: { id: string; label: string; onClick: () => void }[] = [
    ...(installed && onOpenFolder
      ? [{ id: "folder", label: t("Open folder"), onClick: onOpenFolder }]
      : []),
    { id: "controller", label: t("Controller"), onClick: onOpenProfiles },
    { id: "back", label: t("Back"), onClick: onBack },
  ];
  const totalActions = 1 + secondaryActions.length;

  useInputScope({
    id: "console-game-surface",
    onAction: (action) => {
      if (action === "back") {
        onBack();
        return;
      }
      if (action === "prevTab") {
        setTab((current) => TABS[wrapIndex(TABS.indexOf(current), -1, TABS.length)] ?? "overview");
        return;
      }
      if (action === "nextTab") {
        setTab((current) => TABS[wrapIndex(TABS.indexOf(current), 1, TABS.length)] ?? "overview");
        return;
      }
      if (action === "down") {
        setFocusArea("tabs");
        return;
      }
      if (action === "up") {
        setFocusArea("actions");
        return;
      }
      if (action === "left" || action === "right") {
        const delta = action === "left" ? -1 : 1;
        if (focusArea === "tabs") {
          setTab((current) => TABS[wrapIndex(TABS.indexOf(current), delta, TABS.length)] ?? "overview");
        } else {
          setActionIndex((current) => wrapIndex(current, delta, totalActions));
        }
        return;
      }
      if (action === "confirm") {
        if (focusArea === "actions") {
          if (actionIndex === 0) onPrimary();
          else secondaryActions[actionIndex - 1]?.onClick();
        } else if (tab === "controller") {
          onOpenProfiles();
        }
      }
    },
    onKey: (event) => {
      if (event.key === "Escape" || event.key === "Backspace") {
        event.preventDefault();
        onBack();
        return true;
      }
      if (event.key === "ArrowDown") {
        event.preventDefault();
        setFocusArea("tabs");
        return true;
      }
      if (event.key === "ArrowUp") {
        event.preventDefault();
        setFocusArea("actions");
        return true;
      }
      if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
        event.preventDefault();
        const delta = event.key === "ArrowLeft" ? -1 : 1;
        if (focusArea === "tabs") {
          setTab((current) => TABS[wrapIndex(TABS.indexOf(current), delta, TABS.length)] ?? "overview");
        } else {
          setActionIndex((current) => wrapIndex(current, delta, totalActions));
        }
        return true;
      }
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        if (focusArea === "actions") {
          if (actionIndex === 0) onPrimary();
          else secondaryActions[actionIndex - 1]?.onClick();
        } else if (tab === "controller") {
          onOpenProfiles();
        }
        return true;
      }
      return false;
    },
  });

  const year = formatYear(game.releaseDate);
  const rating = formatRating(game.rating);
  const playtime = formatMinutes(game.minutesPlayed);
  const version = detail?.versions[0];

  return (
    <section
      className="flex min-h-0 flex-1 flex-col overflow-y-auto overscroll-contain py-6 bp-pad-x"
      aria-label={game.title}
      data-console-game={game.id}
    >
      <div className="flex flex-col gap-6 sm:flex-row sm:items-end">
        <div className="w-[calc(var(--bp-u)*220)] shrink-0">
          <GameCover title={game.title} coverUrl={game.coverUrl} showType={false} />
        </div>

        <div className="flex min-w-0 flex-1 flex-col gap-3">
          <h1 className="line-clamp-2 max-w-[20ch] text-bp-hero font-semibold leading-[1.02] tracking-[-0.02em] text-ink">
            {game.title}
          </h1>

          <div className="flex flex-wrap items-center gap-x-5 gap-y-1 text-bp-meta text-ink-2">
            {year && <span className="num">{year}</span>}
            <span>{stateLabel(game.state)}</span>
            <span className="num">{formatBytes(game.totalSize)}</span>
            {rating && <span className="num">{rating}/100</span>}
            {playtime && <span className="num">{playtime}</span>}
            {installed && <span className="text-ok">{t("Installed")}</span>}
          </div>

          <div className="mt-2 flex flex-wrap items-center gap-3">
            <button
              type="button"
              data-gamepad-focus={focusArea === "actions" && actionIndex === 0 ? "true" : undefined}
              onPointerEnter={() => {
                setFocusArea("actions");
                setActionIndex(0);
              }}
              onClick={onPrimary}
              className={cn(
                "bp-button inline-flex min-h-[calc(var(--bp-u)*72)] items-center gap-3 rounded-[var(--bp-radius-object)] px-7 text-bp-body font-semibold",
                "bg-signal text-on-signal",
              )}
            >
              <Glyph name={primaryGlyph} className="size-[calc(var(--bp-u)*28)] shrink-0" />
              <span>{primaryLabel}</span>
            </button>

            {secondaryActions.map((item, idx) => {
              const selected = focusArea === "actions" && actionIndex === idx + 1;
              return (
                <button
                  key={item.id}
                  type="button"
                  data-gamepad-focus={selected ? "true" : undefined}
                  onPointerEnter={() => {
                    setFocusArea("actions");
                    setActionIndex(idx + 1);
                  }}
                  onClick={item.onClick}
                  className={cn(
                    "bp-button inline-flex min-h-[calc(var(--bp-u)*72)] items-center gap-2.5 rounded-[var(--bp-radius-object)] border px-5 text-bp-body font-medium",
                    selected
                      ? "border-signal bg-surface-2 text-ink"
                      : "border-line-2 bg-surface text-ink-2",
                  )}
                >
                  <span>{item.label}</span>
                </button>
              );
            })}
          </div>
        </div>
      </div>

      <nav
        aria-label={t("Sections")}
        className="mt-7 flex flex-wrap gap-2 border-b border-line pb-3"
      >
        {TABS.map((entry) => {
          const active = tab === entry;
          return (
            <button
              key={entry}
              type="button"
              aria-current={active ? "page" : undefined}
              data-gamepad-focus={focusArea === "tabs" && active ? "true" : undefined}
              onClick={() => {
                setFocusArea("tabs");
                setTab(entry);
              }}
              className={cn(
                "relative rounded-[var(--bp-radius-object)] px-5 py-2.5 text-bp-body font-medium transition-colors",
                active ? "bg-surface-2 text-ink" : "text-ink-3 hover:text-ink-2",
              )}
            >
              {tabLabel(entry)}
              {active && (
                <span
                  aria-hidden="true"
                  className="pointer-events-none absolute inset-x-4 bottom-0 h-[calc(var(--bp-u)*3)] bg-signal"
                />
              )}
            </button>
          );
        })}
      </nav>

      <div className="mt-5 max-w-[980px] rounded-[var(--bp-radius-surface)] border border-line bg-surface p-6">
        {tab === "overview" && (
          <div className="flex flex-col gap-4">
            <p className="text-bp-body leading-relaxed text-ink-2">
              {detail?.description || t("No description available.")}
            </p>
            <dl className="grid gap-4 border-t border-line pt-4 sm:grid-cols-3">
              <div>
                <dt className="text-bp-meta text-ink-3">{t("Version")}</dt>
                <dd className="num mt-1 text-bp-body text-ink">{version?.version ?? "—"}</dd>
              </div>
              <div>
                <dt className="text-bp-meta text-ink-3">{t("Size")}</dt>
                <dd className="num mt-1 text-bp-body text-ink">{formatBytes(version?.size ?? game.totalSize)}</dd>
              </div>
              <div>
                <dt className="text-bp-meta text-ink-3">{t("Genres")}</dt>
                <dd className="mt-1 text-bp-body text-ink">{game.genres.join(", ") || "—"}</dd>
              </div>
            </dl>
            {installed?.installPath && (
              <p className="border-t border-line pt-3 text-bp-meta text-ink-3">
                {t("Install folder")}: <span className="num text-ink-2">{installed.installPath}</span>
              </p>
            )}
          </div>
        )}

        {tab === "saves" && (
          <div className="flex flex-col gap-3">
            {!online ? (
              <p className="text-bp-body text-ink-2">
                {t("Cloud saves require a connection to the server. Local saves remain on this device.")}
              </p>
            ) : savesQuery.isPending ? (
              <p className="text-bp-body text-ink-3">{t("Loading…")}</p>
            ) : (savesQuery.data?.length ?? 0) === 0 ? (
              <p className="text-bp-body text-ink-2">{t("No cloud saves yet.")}</p>
            ) : (
              <ul className="flex flex-col gap-2">
                {(savesQuery.data ?? []).slice(0, 6).map((save) => (
                  <li
                    key={save.id}
                    className="flex items-center justify-between gap-4 border-b border-line py-2 last:border-0"
                  >
                    <span className="truncate text-bp-body text-ink">{save.label || save.fileName}</span>
                    <span className="num shrink-0 text-bp-meta text-ink-3">{formatBytes(save.size)}</span>
                  </li>
                ))}
              </ul>
            )}
          </div>
        )}

        {tab === "artwork" && (
          <div className="grid gap-6 sm:grid-cols-[200px_1fr]">
            <GameCover title={game.title} coverUrl={game.coverUrl} showType={false} />
            <div className="flex flex-col justify-center gap-2">
              <p className="text-bp-title font-semibold text-ink">{game.title}</p>
              <p className="text-bp-meta text-ink-2">
                {game.coverUrl
                  ? t("Poster artwork is synced from the library server.")
                  : t("Using the generated Ludex archive plate.")}
              </p>
            </div>
          </div>
        )}

        {tab === "controller" && (
          <div className="flex flex-col gap-4">
            <div className="flex flex-wrap items-center justify-between gap-4">
              <div>
                <p className="text-bp-title font-semibold text-ink">{profile.name}</p>
                <p className="mt-1 text-bp-meta text-ink-2">{padFamilyName}</p>
              </div>
              <button
                type="button"
                onClick={onOpenProfiles}
                className="bp-button inline-flex min-h-[calc(var(--bp-u)*64)] items-center gap-2 rounded-[var(--bp-radius-object)] border border-line-2 bg-surface-2 px-5 text-bp-body font-medium text-ink"
              >
                <Glyph name="pad" className="size-[calc(var(--bp-u)*24)]" />
                <span>{t("Controller profile")}</span>
              </button>
            </div>
          </div>
        )}
      </div>
    </section>
  );
}
