import { useEffect, useMemo, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { useReducedMotion } from "motion/react";
import { Download, Gamepad2, Play, X } from "lucide-react";
import { api } from "@/lib/api";
import { downloadGame } from "@/lib/download";
import { errorMessage } from "@/lib/http";
import { formatBytes, formatMinutes, formatRating, formatRelative, formatYear, stateLabel, formatPercent } from "@/lib/format";
import { isNative } from "@/lib/native";
import { useAllGames, useInstalledMap, useLaunchGame } from "@/lib/hooks";
import { useGamepad, type GamepadAction } from "@/lib/gamepad";
import { useDownloads } from "@/state/downloads";
import { useToast } from "@/state/toasts";
import { Wordmark } from "@/components/brand";
import { ArtPlate, GameCover } from "@/components/art/cover";
import { InstallDialog } from "@/components/game/install-dialog";
import { Button, Kbd, Skeleton } from "@/components/ui/controls";
import { cn } from "@/lib/cn";
import { t , tn } from "@/lib/i18n";

const clamp = (value: number, min: number, max: number) => Math.min(Math.max(value, min), max);

/**
 * Big Picture: the same archive language at ten feet. Full-screen, dark, one
 * shelf at a time, driven by arrows, Enter and a gamepad.
 */
export function BigPictureScreen() {
  const navigate = useNavigate();
  const { push } = useToast();
  const { data, isPending } = useAllGames();
  const installed = useInstalledMap();
  const downloads = useDownloads();
  const launch = useLaunchGame();
  const [shelfIndex, setShelfIndex] = useState(0);
  const [index, setIndex] = useState(0);
  const [installOpen, setInstallOpen] = useState(false);
  const [clock, setClock] = useState(() => new Date());
  const reduceMotion = useReducedMotion();

  const games = data?.items ?? [];

  const shelves = useMemo(() => {
    const recent = games
      .filter((game) => game.lastPlayedAt)
      .sort((a, b) => Date.parse(b.lastPlayedAt ?? "") - Date.parse(a.lastPlayedAt ?? ""));
    return [
      { id: "recent", label: t("Recently played"), games: recent },
      { id: "all", label: t("All games"), games },
    ].filter((shelf) => shelf.games.length > 0);
  }, [games]);

  const shelf = shelves[Math.min(shelfIndex, shelves.length - 1)];
  const shelfGames = shelf?.games ?? [];
  const selected = shelfGames[Math.min(index, shelfGames.length - 1)];

  const detail = useQuery({
    queryKey: ["game", selected?.id],
    queryFn: () => api.getGame(selected!.id),
    enabled: Boolean(selected),
    staleTime: 60_000,
  });
  const version = detail.data?.versions[0];
  const installedGame = selected ? installed.get(selected.id) : undefined;
  const job = selected ? downloads.jobForGame(selected.id) : undefined;
  const pct = job?.bytesTotal ? Math.min(100, (job.bytesDone / job.bytesTotal) * 100) : job ? 0 : null;

  useEffect(() => {
    const timer = setInterval(() => setClock(new Date()), 30_000);
    return () => clearInterval(timer);
  }, []);

  // Big Picture owns the OS window in the desktop app: enter real fullscreen on
  // mount and hand the window back on the way out. The browser build only gets
  // the full-viewport layout, and a refused fullscreen never traps the user.
  useEffect(() => {
    if (!isNative()) return;
    let unmounted = false;
    let entered = false;

    const enter = async () => {
      try {
        const { getCurrentWindow } = await import("@tauri-apps/api/window");
        const win = getCurrentWindow();
        if ((await win.isFullscreen()) || unmounted) return;
        await win.setFullscreen(true);
        entered = true;
      } catch (cause) {
        console.warn("Big Picture could not enter fullscreen", cause);
        if (!unmounted) {
          push({
            title: "Fullscreen unavailable",
            description: "Staying in the in-app Big Picture view.",
            tone: "default",
          });
        }
      }
    };

    const start = enter();

    // Every exit (Esc, Backspace, gamepad B, the Exit button, a route change or
    // launching a game) unmounts this screen. Cleanup waits for a still-running
    // enter so it can restore only what Ludex itself took over.
    return () => {
      unmounted = true;
      void start.then(async () => {
        if (!entered) return;
        try {
          const { getCurrentWindow } = await import("@tauri-apps/api/window");
          await getCurrentWindow().setFullscreen(false);
        } catch (cause) {
          console.warn("Big Picture could not leave fullscreen", cause);
        }
      });
    };
  }, [push]);

  useEffect(() => {
    setIndex((current) => clamp(current, 0, Math.max(0, shelfGames.length - 1)));
  }, [shelfGames.length]);

  const railRef = useRef<HTMLDivElement>(null);
  const cardRefs = useRef<Array<HTMLButtonElement | null>>([]);

  useEffect(() => {
    cardRefs.current[index]?.scrollIntoView({
      behavior: reduceMotion ? "auto" : "smooth",
      inline: "center",
      block: "nearest",
    });
  }, [index, shelfIndex, reduceMotion]);

  const exit = () => {
    if (window.history.length > 1) navigate(-1);
    else navigate("/");
  };

  const move = (delta: number) => setIndex((current) => clamp(current + delta, 0, shelfGames.length - 1));
  const switchShelf = (delta: number) => {
    if (shelves.length < 2) return;
    setShelfIndex((current) => {
      const next = clamp(current + delta, 0, shelves.length - 1);
      setIndex(0);
      return next;
    });
  };

  const download = async () => {
    if (!selected) return;
    try {
      await downloadGame(selected.id, version?.id);
    } catch (cause) {
      push({ title: t("Download did not start"), description: errorMessage(cause), tone: "danger" });
    }
  };

  const confirm = () => {
    if (!selected) return;
    if (installedGame) {
      launch.mutate({ id: selected.id, title: selected.title, executable: installedGame.executable ?? detail.data?.launch.executable });
    } else if (job) {
      navigate("/downloads");
    } else if (isNative()) {
      setInstallOpen(true);
    } else {
      void download();
    }
  };

  const actions = useRef({ left: () => {}, right: () => {}, up: () => {}, down: () => {}, confirm: () => {}, back: () => {}, downloads: () => {} });
  actions.current = {
    left: () => move(-1),
    right: () => move(1),
    up: () => switchShelf(-1),
    down: () => switchShelf(1),
    confirm,
    back: exit,
    downloads: () => navigate("/downloads"),
  };

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (document.querySelector('[role="dialog"][aria-modal="true"]')) return;
      if (event.key === "ArrowLeft") {
        event.preventDefault();
        actions.current.left();
      } else if (event.key === "ArrowRight") {
        event.preventDefault();
        actions.current.right();
      } else if (event.key === "ArrowUp") {
        event.preventDefault();
        actions.current.up();
      } else if (event.key === "ArrowDown") {
        event.preventDefault();
        actions.current.down();
      } else if (event.key === "Enter") {
        event.preventDefault();
        actions.current.confirm();
      } else if (event.key === "Escape" || event.key === "Backspace") {
        event.preventDefault();
        actions.current.back();
      } else if (event.key.toLowerCase() === "d") {
        actions.current.downloads();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // Gamepad: d-pad/stick moves, A confirms, B exits, Y opens downloads and the
  // shoulders change shelf (the same action as up/down).
  const onGamepad = (action: GamepadAction) => {
    if (action === "left") move(-1);
    else if (action === "right") move(1);
    else if (action === "up" || action === "prevTab") switchShelf(-1);
    else if (action === "down" || action === "nextTab") switchShelf(1);
    else if (action === "confirm") confirm();
    else if (action === "back") exit();
    else if (action === "downloads") navigate("/downloads");
  };
  const gamepad = useGamepad(onGamepad, { enabled: !installOpen });

  const time = new Intl.DateTimeFormat("en-GB", { hour: "2-digit", minute: "2-digit" }).format(clock);

  if (isPending) {
    return (
      <div className="flex h-dvh flex-col bg-bg-deep">
        <div className="h-16 border-b border-line" />
        <div className="flex flex-1 items-end p-10">
          <Skeleton className="h-16 w-1/2" />
        </div>
        <div className="flex gap-3 border-t border-line p-6">
          {Array.from({ length: 7 }).map((_, i) => (
            <Skeleton key={i} className="aspect-[3/4] w-[132px]" />
          ))}
        </div>
      </div>
    );
  }

  if (!selected) {
    return (
      <div className="flex h-dvh flex-col items-center justify-center gap-5 bg-bg-deep text-center">
        <Wordmark height={22} />
        <p className="text-[15px] text-ink-2">{t("The library is empty. Add a game first.")}</p>
        <Button variant="primary" onClick={exit}>
          {t("Exit Big Picture")}
        </Button>
      </div>
    );
  }

  const primary = installedGame ? (
    <Button
      variant="primary"
      size="lg"
      icon={<Play className="size-4" />}
      onClick={() =>
        launch.mutate({ id: selected.id, title: selected.title, executable: installedGame.executable ?? detail.data?.launch.executable })
      }
    >
      {t("Play")}
    </Button>
  ) : job ? (
    <Button variant="default" size="lg" icon={<Download className="size-4" />} onClick={() => navigate("/downloads")}>
      {t("Installing {percent}", { percent: pct !== null ? formatPercent(pct) : "" })}
    </Button>
  ) : isNative() ? (
    <Button variant="primary" size="lg" icon={<Download className="size-4" />} disabled={!version} onClick={() => setInstallOpen(true)}>
      {t("Install")}
    </Button>
  ) : (
    <Button variant="primary" size="lg" icon={<Download className="size-4" />} onClick={() => void download()}>
      {t("Download")}
    </Button>
  );

  return (
    <div className="force-dark flex h-dvh flex-col overflow-hidden bg-bg-deep text-ink">
      <div className="grain" aria-hidden="true" />

      <header className="relative z-10 flex h-16 shrink-0 items-center gap-4 border-b border-line px-6 lg:px-8">
        <Wordmark height={17} />
        <span className="hidden font-mono text-[10px] uppercase tracking-[0.16em] text-ink-3 sm:inline">{t("Big Picture")}</span>
        <div className="ml-auto flex items-center gap-3">
          {downloads.activeCount > 0 && (
            <button
              type="button"
              onClick={() => navigate("/downloads")}
              className="flex h-8 items-center gap-2 border border-line px-2.5 text-[11.5px] text-ink-2 transition-colors hover:border-line-2 hover:text-ink"
            >
              <Download className="size-3.5 text-signal-text" aria-hidden="true" />
              <span>{tn(downloads.activeCount, "1 downloading", "{count} downloading")}</span>
              {downloads.aggregate?.percent !== null && downloads.aggregate && (
                <span className="num text-ink-3">{formatPercent(downloads.aggregate.percent)}</span>
              )}
            </button>
          )}
          <span className="num text-[12px] text-ink-2">{time}</span>
          <Button variant="ghost" size="sm" icon={<X className="size-3.5" />} onClick={exit}>
            {t("Exit")}
          </Button>
        </div>
      </header>

      <main className="relative z-10 flex min-h-0 flex-1 flex-col">
        <section className="relative flex min-h-0 flex-1 items-center overflow-hidden">
          <div className="absolute inset-0 opacity-25">
            <ArtPlate
              title={selected.title}
              artUrl={detail.data?.backgroundUrl ?? selected.backgroundUrl}
              className="h-full w-full"
            />
          </div>
          <div
            className="absolute inset-0"
            style={{
              background:
                "linear-gradient(180deg, rgba(5,5,5,0.3) 0%, rgba(5,5,5,0.12) 45%, #050505 100%)",
            }}
          />

          <div className="relative flex w-full items-center gap-6 px-6 py-6 lg:gap-10 lg:px-12">
            <div className="w-[148px] shrink-0 sm:w-[200px] lg:w-[264px] xl:w-[296px]">
              <div className="shadow-[0_44px_90px_-32px_rgba(0,0,0,0.85)]">
                <GameCover
                  title={selected.title}
                  coverUrl={selected.coverUrl}
                  index={selected.id}
                  year={formatYear(selected.releaseDate)}
                  genre={selected.genres[0]}
                />
              </div>
            </div>

            <div className="flex min-w-0 flex-1 flex-col gap-3">
              <span className="flex items-center gap-2.5 font-mono text-[10px] uppercase tracking-[0.16em] text-[#ffc61a]">
                <span className="h-px w-8 bg-[#ffc61a]" aria-hidden="true" />
                {selected.lastPlayedAt ? t("Continue playing") : t("From the library")}
              </span>
              <h1 className="max-w-[20ch] text-[clamp(2rem,4.4vw,4.35rem)] font-semibold leading-[0.92] tracking-[-0.04em] text-[#ece8de]">
                {selected.title}
              </h1>
              <div className="flex flex-wrap items-center gap-x-5 gap-y-1 font-mono text-[11px] text-[#a8a396]">
                <span>{formatYear(selected.releaseDate)}</span>
                <span>{stateLabel(selected.state)}</span>
                {formatRating(selected.rating) && <span>{formatRating(selected.rating)} / 100</span>}
                {formatMinutes(selected.minutesPlayed) && <span>{t("{duration} played", { duration: formatMinutes(selected.minutesPlayed) ?? "" })}</span>}
                {selected.lastPlayedAt && <span>{t("Last played")} {formatRelative(selected.lastPlayedAt)}</span>}
                {installedGame && <span className="text-[#79c99f]">{t("Installed")}</span>}
              </div>
              {detail.data?.description && (
                <p className="line-clamp-2 max-w-[64ch] text-[14px] leading-relaxed text-[#c9c4b8]">{detail.data.description}</p>
              )}
              {job && (
                <p className="flex flex-wrap items-baseline gap-x-3 font-mono text-[10.5px] text-[#a8a396]">
                  <span className="text-[#ffc61a]">{pct !== null ? formatPercent(pct) : t("Preparing")}</span>
                  {job.bytesTotal && <span>{t("{done} of {total}", { done: formatBytes(job.bytesDone), total: formatBytes(job.bytesTotal) })}</span>}
                  {job.speedBps > 0 && <span>{formatBytes(job.speedBps, 0)}/s</span>}
                </p>
              )}
              <div className="mt-1 flex flex-wrap items-center gap-2">
                {primary}
                <Button variant="default" onClick={() => navigate(`/game/${selected.id}`)}>
                  {t("Game page")}
                </Button>
              </div>
            </div>
          </div>
        </section>

        <section className="relative shrink-0 border-t border-line px-4 pb-3 pt-4 lg:px-6" aria-label={t("Games")}>
          <div className="mb-2 flex items-baseline justify-between px-2">
            <span className="text-[12px] text-ink-3">{shelf?.label}</span>
            <span className="num text-[10.5px] text-ink-3">
              {index + 1} / {shelfGames.length}
            </span>
          </div>
          <div ref={railRef} className="nav-scroll flex gap-3 overflow-x-auto px-3 pb-3">
            {shelfGames.map((game, position) => {
              const isSelected = position === index;
              return (
                <button
                  key={game.id}
                  type="button"
                  ref={(element) => {
                    cardRefs.current[position] = element;
                  }}
                  onClick={() => setIndex(position)}
                  aria-current={isSelected ? "true" : undefined}
                  aria-label={game.title}
                  className={cn(
                    "relative w-[118px] shrink-0 rounded-[2px] text-left transition-[transform,opacity] duration-150 lg:w-[132px]",
                    isSelected ? "scale-[1.03] opacity-100" : "opacity-70 hover:opacity-100",
                  )}
                >
                  <GameCover title={game.title} coverUrl={game.coverUrl} index={game.id} showType={false} />
                  {isSelected && (
                    <span
                      className="pointer-events-none absolute -inset-[4px] z-10 rounded-[2px] ring-2 ring-signal ring-offset-2 ring-offset-[#0a0a09]"
                      aria-hidden="true"
                    />
                  )}
                  <span className={cn("mt-2 block truncate text-[12px]", isSelected ? "text-ink" : "text-ink-3")}>
                    {game.title}
                  </span>
                  <span className="num block text-[9.5px] text-ink-3">{formatYear(game.releaseDate)}</span>
                  {installed.has(game.id) && (
                    <span className="absolute left-1.5 top-1.5 h-1.5 w-1.5 rounded-full bg-ok" aria-hidden="true" />
                  )}
                </button>
              );
            })}
          </div>
        </section>

        <footer className="relative flex h-10 shrink-0 flex-wrap items-center gap-x-5 gap-y-1 border-t border-line px-6 text-[10.5px] text-ink-3 lg:px-8">
          <span className="flex items-center gap-1.5">
            <Kbd>←</Kbd>
            <Kbd>→</Kbd>
            {t("select")}
          </span>
          <span className="flex items-center gap-1.5">
            <Kbd>↑</Kbd>
            <Kbd>↓</Kbd>
            {t("shelf")}
          </span>
          <span className="flex items-center gap-1.5">
            <Kbd>↵</Kbd>
            {t("play or install")}
          </span>
          <span className="flex items-center gap-1.5">
            <Kbd>esc</Kbd>
            {t("exit")}
          </span>
          <span className="ml-auto flex items-center gap-1.5">
            <Gamepad2 className="size-3.5" aria-hidden="true" />
            {gamepad.connected
              ? t("Gamepad: stick or d-pad, A, B, Y, LB/RB shelf")
              : t("Keyboard only — connect a controller for couch navigation")}
          </span>
        </footer>
      </main>

      {version && detail.data && (
        <InstallDialog open={installOpen} onOpenChange={setInstallOpen} game={detail.data} version={version} />
      )}
    </div>
  );
}
