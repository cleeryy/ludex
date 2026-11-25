import { useMemo, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import {
  Bookmark,
  ChevronLeft,
  ChevronRight,
  Clock,
  Download,
  FolderOpen,
  HardDrive,
  Library,
  Play,
  Sparkles,
  Trophy,
} from "lucide-react";
import { api } from "@/lib/api";
import { cn } from "@/lib/cn";
import { downloadGame } from "@/lib/download";
import { errorMessage } from "@/lib/http";
import {
  formatBytes,
  formatMinutes,
  formatPercent,
  formatRating,
  formatRelative,
  formatYear,
  stateLabel,
} from "@/lib/format";
import { isNative } from "@/lib/native";
import { installsAvailable } from "@/lib/install";
import {
  useAllGames,
  useBookmark,
  useInstalledMap,
  useLaunchGame,
  useRecentGames,
  useUpdateGame,
  useUpdateMap,
} from "@/lib/hooks";
import { useIsEditor } from "@/state/auth";
import { useJob } from "@/state/downloads";
import { useToast } from "@/state/toasts";
import { ArtPlate, GameCover } from "@/components/art/cover";
import { ThemeScope } from "@/components/theme/theme-scope";
import { GameCard } from "@/components/game/game-card";
import { InstallDialog } from "@/components/game/install-dialog";
import { GettingStartedCard } from "@/components/onboarding/getting-started";
import { EmptyState, ErrorState, SectionHeader } from "@/components/page";
import { Button, Led, Skeleton } from "@/components/ui/controls";
import { t, tn } from "@/lib/i18n";
import { qk } from "@/lib/query-keys";

/** Source label for an imported or adopted entry (#13, #116, #129). */
function sourceLabel(source: string): string {
  switch (source) {
    case "adopted":
      return t("Adopted");
    case "steam":
      return t("Steam");
    case "epic":
      return t("Epic");
    case "gog":
      return t("GOG");
    case "ea":
      return t("EA app");
    case "ubisoft":
      return t("Ubisoft");
    case "xbox":
      return t("Xbox");
    case "itch":
      return t("itch.io");
    case "heroic":
      return t("Heroic");
    case "lutris":
      return t("Lutris");
    default:
      return source;
  }
}

const EMPTY_GAMES: never[] = [];

export function HomeScreen() {
  const navigate = useNavigate();
  const { push } = useToast();
  const isEditor = useIsEditor();
  const installed = useInstalledMap();
  const launch = useLaunchGame();
  const updates = useUpdateMap();
  const updateGame = useUpdateGame();
  const recentOnDevice = useRecentGames(8);
  const bookmark = useBookmark();
  const [installOpen, setInstallOpen] = useState(false);
  const [spotlightIndex, setSpotlightIndex] = useState(0);

  const { data, isPending, isError, error, refetch } = useAllGames();
  const games = data?.items ?? EMPTY_GAMES;
  const collectionsQuery = useQuery({ queryKey: qk.collections.root, queryFn: () => api.listCollections() });
  const shelves = (collectionsQuery.data ?? []).slice(0, 4);

  const played = useMemo(
    () =>
      games
        .filter((game) => game.lastPlayedAt)
        .sort((a, b) => Date.parse(b.lastPlayedAt ?? "") - Date.parse(a.lastPlayedAt ?? "")),
    [games],
  );

  // Up to 5 spotlight candidates: recently played installed -> recently played -> highest rated -> rest.
  const featured = useMemo(() => {
    const out: typeof games = [];
    const seen = new Set<number>();
    const add = (list: typeof games) => {
      for (const game of list) {
        if (out.length >= 5) return;
        if (seen.has(game.id)) continue;
        seen.add(game.id);
        out.push(game);
      }
    };
    add(played.filter((game) => installed.has(game.id)));
    add(played);
    add([...games].filter((game) => installed.has(game.id)).sort((a, b) => (b.rating ?? 0) - (a.rating ?? 0)));
    add([...games].sort((a, b) => (b.rating ?? 0) - (a.rating ?? 0)));
    add(games);
    return out;
  }, [played, installed, games]);

  const safeSpotlight = featured.length > 0 ? spotlightIndex % featured.length : 0;
  const hero = featured[safeSpotlight];

  const heroDetail = useQuery({
    queryKey: qk.game.detail(hero?.id),
    queryFn: () => api.getGame(hero!.id),
    enabled: Boolean(hero),
    staleTime: 60_000,
  });

  const version = heroDetail.data?.versions[0];
  const heroInstalled = hero ? installed.get(hero.id) : undefined;
  const heroJob = useJob(hero?.id ?? -1);
  const heroPct = heroJob?.bytesTotal ? Math.min(100, (heroJob.bytesDone / heroJob.bytesTotal) * 100) : heroJob ? 0 : null;

  const recent = played.filter((game) => game.id !== hero?.id).slice(0, 10);

  const gamesById = useMemo(() => new Map(games.map((game) => [game.id, game])), [games]);
  const deviceGames = (recentOnDevice.data ?? []).slice(0, 6);

  const summary = useMemo(() => {
    let totalSize = 0;
    let totalMinutes = 0;
    let playingCount = 0;
    let completedCount = 0;
    let bookmarkedCount = 0;
    let playedCount = 0;
    for (const game of games) {
      totalSize += game.totalSize;
      totalMinutes += game.minutesPlayed;
      if (game.minutesPlayed > 0 || game.lastPlayedAt) playedCount += 1;
      if (game.state === "playing") playingCount += 1;
      if (game.state === "completed") completedCount += 1;
      if (game.bookmarked) bookmarkedCount += 1;
    }
    let installedBytes = 0;
    for (const entry of installed.values()) {
      installedBytes += entry.sizeBytes;
    }
    return {
      totalSize,
      totalMinutes,
      playingCount,
      completedCount,
      bookmarkedCount,
      playedCount,
      installedBytes,
    };
  }, [games, installed]);

  const download = async () => {
    if (!hero) return;
    try {
      await downloadGame(hero.id, version?.id);
    } catch (cause) {
      push({ title: t("Download did not start"), description: errorMessage(cause), tone: "danger" });
    }
  };

  if (isPending) {
    return (
      <div className="page page-in pt-6">
        <Skeleton className="h-[420px] w-full rounded-[var(--r-lg)]" />
        <div className="mt-5 grid grid-cols-2 gap-3 lg:grid-cols-4">
          {Array.from({ length: 4 }).map((_, index) => (
            <Skeleton key={index} className="h-20 w-full rounded-[var(--r-md)]" />
          ))}
        </div>
        <div className="mt-10 grid grid-cols-2 gap-x-6 gap-y-8 sm:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5">
          {Array.from({ length: 5 }).map((_, index) => (
            <div key={index}>
              <Skeleton className="aspect-[2/3] w-full" />
              <Skeleton className="mt-3 h-3.5 w-4/5" />
            </div>
          ))}
        </div>
      </div>
    );
  }

  if (isError) {
    return (
      <div className="page page-in pt-6">
        <ErrorState
          title={t("Home didn’t load")}
          message={errorMessage(error)}
          onRetry={() => void refetch()}
        />
      </div>
    );
  }

  if (!hero) {
    return (
      <div className="page page-in pt-6">
        <EmptyState
          title={t("Nothing on the shelf yet")}
          description={
            isEditor
              ? t("The games folder is empty. Paste a download link and Ludex will fetch it, verify the archive and index it for you.")
              : t("This library is empty. An administrator can add games from the server, or drop archives into the games folder.")
          }
        >
          {isEditor ? (
            <Button variant="primary" icon={<Download className="size-4" />} onClick={() => navigate("/admin/ingest")}>
              {t("Add games")}
            </Button>
          ) : null}
        </EmptyState>
      </div>
    );
  }

  const primary = heroInstalled ? (
    <Button
      variant="primary"
      size="lg"
      icon={<Play className="size-4" />}
      onClick={() =>
        launch.mutate({
          id: hero.id,
          title: hero.title,
          executable: heroInstalled.executable ?? heroDetail.data?.launch.executable,
        })
      }
    >
      {t("Play")}
    </Button>
  ) : heroJob ? (
    <Button variant="default" size="lg" icon={<Download className="size-4" />} onClick={() => navigate("/downloads")}>
      {t("Installing {percent}", { percent: heroPct !== null ? formatPercent(heroPct) : "…" })}
    </Button>
  ) : isNative() ? (
    <Button
      variant="primary"
      size="lg"
      icon={<Download className="size-4" />}
      disabled={!version}
      onClick={() => setInstallOpen(true)}
    >
      {t("Install")}
    </Button>
  ) : (
    <Button variant="primary" size="lg" icon={<Download className="size-4" />} onClick={() => void download()}>
      {t("Download")}
    </Button>
  );

  const playtime = formatMinutes(hero.minutesPlayed);
  const rating = formatRating(hero.rating);
  const totalPlaytime = formatMinutes(summary.totalMinutes);
  const stepSpotlight = (delta: number) => {
    if (featured.length < 2) return;
    setSpotlightIndex((current) => (current + delta + featured.length) % featured.length);
  };

  return (
    <div className="page page-in pt-5">
      <GettingStartedCard className="mb-6" />

      {/* Flagship multi-game spotlight hero */}
      <ThemeScope
        as="section"
        aria-label={t("Featured")}
        className="relative overflow-hidden rounded-[var(--r-lg)] border border-line-2 bg-stage shadow-hero"
      >
        <ArtPlate
          title={hero.title}
          artUrl={heroDetail.data?.backgroundUrl ?? hero.backgroundUrl}
          fallbackUrls={[heroDetail.data?.screenshots?.[0], heroDetail.data?.coverUrl, hero.coverUrl]}
          className="absolute inset-0 h-full w-full"
        />
        <div className="absolute inset-0 hero-shade" aria-hidden="true" />

        <div className="relative flex min-h-[380px] flex-col justify-between gap-6 p-6 sm:min-h-[430px] sm:p-8">
          <div className="flex items-center justify-between gap-4">
            <span className="inline-flex items-center gap-2 rounded-[var(--r-sm)] border border-sheen/15 bg-stage/75 px-2.5 py-1 text-micro font-semibold uppercase tracking-[0.12em] text-signal">
              <span className="h-[2px] w-4 bg-signal" aria-hidden="true" />
              {hero.lastPlayedAt ? t("Continue playing") : t("Featured")}
            </span>

            {featured.length > 1 && (
              <div className="flex items-center gap-1.5">
                <button
                  type="button"
                  onClick={() => stepSpotlight(-1)}
                  aria-label={t("Previous")}
                  className="grid size-8 place-items-center rounded-[var(--r-sm)] border border-sheen/20 bg-stage/75 text-stage-ink transition-colors hover:border-sheen/40 hover:bg-stage"
                >
                  <ChevronLeft className="rtl:-scale-x-100 size-4" aria-hidden="true" />
                </button>
                <button
                  type="button"
                  onClick={() => stepSpotlight(1)}
                  aria-label={t("Next")}
                  className="grid size-8 place-items-center rounded-[var(--r-sm)] border border-sheen/20 bg-stage/75 text-stage-ink transition-colors hover:border-sheen/40 hover:bg-stage"
                >
                  <ChevronRight className="rtl:-scale-x-100 size-4" aria-hidden="true" />
                </button>
              </div>
            )}
          </div>

          <div className="flex flex-col gap-6 md:flex-row md:items-end md:justify-between">
            <div className="flex min-w-0 items-end gap-6">
              <Link
                to={`/game/${hero.id}`}
                aria-label={t("Open {title}", { title: hero.title })}
                className="cover-host hidden w-[152px] shrink-0 overflow-hidden rounded-[var(--r-md)] shadow-cover md:block"
              >
                <GameCover
                  title={hero.title}
                  coverUrl={hero.coverUrl}
                  index={hero.id}
                  year={formatYear(hero.releaseDate)}
                  genre={hero.genres[0] ?? null}
                />
              </Link>

              <div className="flex min-w-0 flex-col gap-3">
                <h1 className="max-w-[20ch] text-display font-semibold leading-[0.98] tracking-[-0.03em] text-stage-ink sm:text-hero">
                  {hero.title}
                </h1>

                <div className="flex flex-wrap items-center gap-2 text-meta text-stage-ink-2">
                  {heroInstalled && (
                    <span className="inline-flex items-center gap-1.5 rounded-[var(--r-xs)] border border-sheen/15 bg-stage/80 px-2 py-0.5 font-medium text-stage-ok">
                      <Led tone="ok" />
                      {t("Installed")}
                    </span>
                  )}
                  {rating && (
                    <span className="num rounded-[var(--r-xs)] border border-sheen/15 bg-stage/80 px-2 py-0.5 font-semibold text-stage-ink">
                      {rating} / 100
                    </span>
                  )}
                  <span className="num">{formatYear(hero.releaseDate)}</span>
                  <span>{stateLabel(hero.state)}</span>
                  {playtime && <span className="num">{t("{duration} played", { duration: playtime })}</span>}
                  {hero.lastPlayedAt && (
                    <span>{t("Last played {when}", { when: formatRelative(hero.lastPlayedAt) })}</span>
                  )}
                </div>

                {hero.genres.length > 0 && (
                  <div className="flex flex-wrap items-center gap-1.5">
                    {hero.genres.slice(0, 4).map((genre) => (
                      <span
                        key={genre}
                        className="rounded-[var(--r-xs)] border border-sheen/15 bg-stage/65 px-2 py-0.5 text-micro text-stage-ink-2"
                      >
                        {genre}
                      </span>
                    ))}
                  </div>
                )}

                {heroDetail.data?.description && (
                  <p className="line-clamp-2 max-w-[58ch] text-body leading-relaxed text-stage-ink-2">
                    {heroDetail.data.description}
                  </p>
                )}

                <div className="mt-1 flex flex-wrap items-center gap-2.5">
                  {primary}
                  <Button variant="outline" size="lg" onClick={() => navigate(`/game/${hero.id}`)}>
                    {t("Game page")}
                  </Button>
                  {heroJob && (
                    <span className="num text-meta text-stage-ink-2">
                      {heroJob.bytesTotal
                        ? `${formatBytes(heroJob.bytesDone)} of ${formatBytes(heroJob.bytesTotal)}`
                        : t("Preparing")}
                    </span>
                  )}
                </div>
              </div>
            </div>

            {featured.length > 1 && (
              <div className="flex shrink-0 items-center gap-2 self-start md:self-end" role="tablist" aria-label={t("Featured")}>
                {featured.map((entry, idx) => {
                  const selected = idx === safeSpotlight;
                  return (
                    <button
                      key={entry.id}
                      type="button"
                      role="tab"
                      aria-selected={selected}
                      aria-label={entry.title}
                      onClick={() => setSpotlightIndex(idx)}
                      className={cn(
                        "relative w-11 overflow-hidden rounded-[var(--r-xs)] border transition-all duration-[180ms]",
                        selected
                          ? "scale-105 border-signal opacity-100"
                          : "border-sheen/20 opacity-60 hover:border-sheen/45 hover:opacity-100",
                      )}
                    >
                      <GameCover title={entry.title} coverUrl={entry.coverUrl} showType={false} />
                    </button>
                  );
                })}
              </div>
            )}
          </div>
        </div>
      </ThemeScope>

      {/* GOG Galaxy-style launcher telemetry strip */}
      <section aria-label={t("Library")} className="mt-5 grid grid-cols-2 gap-3 lg:grid-cols-4">
        <Link
          to="/library"
          className="group flex items-center gap-3.5 rounded-[var(--r-md)] border border-line bg-surface p-4 transition-colors hover:border-line-2 hover:bg-surface-2"
        >
          <span className="grid size-10 shrink-0 place-items-center rounded-[var(--r-sm)] border border-line bg-bg-deep text-ink-2 transition-colors group-hover:text-signal-text">
            <Library className="size-4" aria-hidden="true" />
          </span>
          <div className="min-w-0">
            <p className="text-micro text-ink-3">{t("Library")}</p>
            <p className="num truncate text-lead font-semibold text-ink">
              {tn(games.length, "1 game", "{count} games")}
            </p>
            <p className="num truncate text-micro text-ink-3">{formatBytes(summary.totalSize)}</p>
          </div>
        </Link>

        {installsAvailable() ? (
          <Link
            to="/library?installed=1"
            className="group flex items-center gap-3.5 rounded-[var(--r-md)] border border-line bg-surface p-4 transition-colors hover:border-line-2 hover:bg-surface-2"
          >
            <span className="grid size-10 shrink-0 place-items-center rounded-[var(--r-sm)] border border-line bg-bg-deep text-ok">
              <HardDrive className="size-4" aria-hidden="true" />
            </span>
            <div className="min-w-0">
              <p className="text-micro text-ink-3">{t("On this device")}</p>
              <p className="num truncate text-lead font-semibold text-ink">
                {t("{count} installed", { count: installed.size })}
              </p>
              <p className="num truncate text-micro text-ink-3">
                {summary.installedBytes > 0 ? formatBytes(summary.installedBytes) : t("Ready to play")}
              </p>
            </div>
          </Link>
        ) : (
          <Link
            to="/bookmarks"
            className="group flex items-center gap-3.5 rounded-[var(--r-md)] border border-line bg-surface p-4 transition-colors hover:border-line-2 hover:bg-surface-2"
          >
            <span className="grid size-10 shrink-0 place-items-center rounded-[var(--r-sm)] border border-line bg-bg-deep text-signal-text">
              <Bookmark className="size-4" aria-hidden="true" />
            </span>
            <div className="min-w-0">
              <p className="text-micro text-ink-3">{t("Bookmarks")}</p>
              <p className="num truncate text-lead font-semibold text-ink">{summary.bookmarkedCount}</p>
              <p className="truncate text-micro text-ink-3">{t("Titles you set aside for later.")}</p>
            </div>
          </Link>
        )}

        <Link
          to="/recent"
          className="group flex items-center gap-3.5 rounded-[var(--r-md)] border border-line bg-surface p-4 transition-colors hover:border-line-2 hover:bg-surface-2"
        >
          <span className="grid size-10 shrink-0 place-items-center rounded-[var(--r-sm)] border border-line bg-bg-deep text-ink-2 transition-colors group-hover:text-signal-text">
            <Clock className="size-4" aria-hidden="true" />
          </span>
          <div className="min-w-0">
            <p className="text-micro text-ink-3">{t("Total playtime")}</p>
            <p className="num truncate text-lead font-semibold text-ink">{totalPlaytime ?? "0m"}</p>
            <p className="num truncate text-micro text-ink-3">
              {tn(summary.playedCount, "1 game", "{count} games")}
            </p>
          </div>
        </Link>

        <Link
          to="/library?state=playing"
          className="group flex items-center gap-3.5 rounded-[var(--r-md)] border border-line bg-surface p-4 transition-colors hover:border-line-2 hover:bg-surface-2"
        >
          <span className="grid size-10 shrink-0 place-items-center rounded-[var(--r-sm)] border border-line bg-bg-deep text-ink-2 transition-colors group-hover:text-signal-text">
            <Trophy className="size-4" aria-hidden="true" />
          </span>
          <div className="min-w-0">
            <p className="text-micro text-ink-3">{t("Progress")}</p>
            <p className="num truncate text-lead font-semibold text-ink">
              {summary.playingCount} {t("Playing")}
            </p>
            <p className="num truncate text-micro text-ink-3">
              {summary.completedCount} {t("Completed")}
            </p>
          </div>
        </Link>
      </section>

      {/* Rich 16:9 installed games cards instead of a plain text table */}
      {deviceGames.length > 0 && (
        <section className="mt-10" aria-labelledby="home-device">
          <SectionHeader
            id="home-device"
            title={t("Ready to play")}
            count={installed.size}
            description={t("Installed here, newest launch first.")}
            actions={
              <Link
                to="/settings?section=storage"
                className="text-label text-ink-3 underline-offset-4 hover:text-ink hover:underline"
              >
                {t("Manage installs")}
              </Link>
            }
          />
          <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
            {deviceGames.map((entry) => {
              const game = gamesById.get(entry.gameId);
              const update = updates.get(entry.gameId);
              const playedMins = game ? formatMinutes(game.minutesPlayed) : null;
              return (
                <article
                  key={entry.gameId}
                  className="cover-host group relative flex flex-col overflow-hidden rounded-[var(--r-md)] border border-line bg-surface transition-all duration-[180ms] hover:border-line-2 hover:bg-surface-2"
                >
                  <div className="relative">
                    <GameCover
                      title={entry.title}
                      coverUrl={game?.backgroundUrl ?? game?.coverUrl ?? null}
                      fallbackUrl={game?.coverUrl ?? null}
                      variant="wide"
                      index={entry.gameId}
                      showType={false}
                      className="rounded-none"
                    />
                    <div
                      className="pointer-events-none absolute inset-0 bg-gradient-to-t from-stage/85 via-transparent to-transparent"
                      aria-hidden="true"
                    />
                    {game && (
                      <Link
                        to={`/game/${entry.gameId}`}
                        className="absolute inset-0 z-[4]"
                        aria-label={t("Open {title}", { title: entry.title })}
                      />
                    )}
                    <div className="pointer-events-none absolute left-2.5 top-2.5 z-[5] flex items-center gap-1.5">
                      <span className="inline-flex items-center gap-1.5 rounded-[var(--r-xs)] border border-sheen/15 bg-stage/85 px-2 py-0.5 text-micro font-medium text-stage-ok">
                        <Led tone="ok" />
                        {entry.source && entry.source !== "ludex" ? sourceLabel(entry.source) : t("Installed")}
                      </span>
                      {update && (
                        <span className="inline-flex items-center gap-1 rounded-[var(--r-xs)] border border-signal/50 bg-stage/85 px-2 py-0.5 text-micro font-medium text-signal">
                          {t("Update")}
                        </span>
                      )}
                    </div>
                  </div>

                  <div className="flex items-center gap-3 p-3.5">
                    <div className="min-w-0 flex-1">
                      {game ? (
                        <Link
                          to={`/game/${entry.gameId}`}
                          className="block truncate text-body font-semibold text-ink transition-colors hover:text-signal-text"
                        >
                          {entry.title}
                        </Link>
                      ) : (
                        <span className="block truncate text-body font-semibold text-ink">{entry.title}</span>
                      )}
                      <p className="mt-1 flex flex-wrap items-center gap-x-2.5 gap-y-0.5 text-micro text-ink-3">
                        {entry.version && <span className="num">{`v${entry.version}`}</span>}
                        <span className="num">{formatBytes(entry.sizeBytes)}</span>
                        {playedMins && <span className="num text-ink-2">{playedMins}</span>}
                        {entry.lastLaunchedAt && <span>{formatRelative(entry.lastLaunchedAt)}</span>}
                      </p>
                    </div>
                    <div className="flex shrink-0 items-center gap-1.5">
                      {update && (
                        <Button
                          variant="ghost"
                          size="sm"
                          icon={<Download className="size-3.5" />}
                          onClick={() => updateGame.mutate({ id: entry.gameId, title: entry.title })}
                        >
                          {t("Update")}
                        </Button>
                      )}
                      <Button
                        variant="primary"
                        size="sm"
                        icon={<Play className="size-3.5" />}
                        onClick={() =>
                          launch.mutate({ id: entry.gameId, title: entry.title, executable: entry.executable })
                        }
                      >
                        {t("Play")}
                      </Button>
                    </div>
                  </div>
                </article>
              );
            })}
          </div>
        </section>
      )}

      {recent.length > 0 && (
        <section className="mt-11" aria-labelledby="home-recent">
          <SectionHeader
            id="home-recent"
            title={t("Recently played")}
            count={recent.length}
            actions={
              <Link to="/recent" className="text-label text-ink-3 underline-offset-4 hover:text-ink hover:underline">
                {t("See all")}
              </Link>
            }
          />
          <div className="nav-scroll -mx-[var(--page-x)] flex gap-5 overflow-x-auto px-[var(--page-x)] pb-2">
            {recent.map((game) => (
              <div key={game.id} className="w-[172px] shrink-0">
                <GameCard
                  game={game}
                  index={game.id}
                  installed={installed.get(game.id)}
                  update={updates.get(game.id)}
                  onBookmark={() => bookmark.mutate({ id: game.id, bookmarked: !game.bookmarked })}
                  onPlay={
                    isNative() && installed.has(game.id)
                      ? () =>
                          launch.mutate({
                            id: game.id,
                            title: game.title,
                            executable: installed.get(game.id)?.executable,
                          })
                      : undefined
                  }
                  onUpdate={
                    isNative() && updates.has(game.id)
                      ? () => updateGame.mutate({ id: game.id, title: game.title })
                      : undefined
                  }
                />
              </div>
            ))}
          </div>
        </section>
      )}

      {shelves.length > 0 && (
        <section className="mt-11" aria-labelledby="home-collections">
          <SectionHeader
            id="home-collections"
            title={t("Collections")}
            count={collectionsQuery.data?.length}
            actions={
              <Link
                to="/collections"
                className="text-label text-ink-3 underline-offset-4 hover:text-ink hover:underline"
              >
                {t("All collections")}
              </Link>
            }
          />
          <div className="grid gap-3.5 sm:grid-cols-2 xl:grid-cols-4">
            {shelves.map((collection) => (
              <Link
                key={collection.id}
                to={`/collections/${collection.id}`}
                className="group flex flex-col justify-between gap-4 rounded-[var(--r-md)] border border-line bg-surface p-4 transition-all duration-[180ms] hover:border-line-2 hover:bg-surface-2"
              >
                <div className="flex items-center justify-between gap-2">
                  <span className="inline-flex items-center gap-1.5 rounded-[var(--r-xs)] border border-line bg-bg-deep px-2 py-0.5 text-micro font-medium text-ink-2">
                    {collection.kind === "smart" ? (
                      <Sparkles className="size-3 text-signal-text" aria-hidden="true" />
                    ) : (
                      <FolderOpen className="size-3 text-ink-3" aria-hidden="true" />
                    )}
                    {collection.kind === "smart" ? t("Smart") : t("Manual")}
                  </span>
                  <span className="num rounded-[var(--r-xs)] bg-bg-deep px-2 py-0.5 text-micro text-ink-2">
                    {tn(collection.gameCount, "1 game", "{count} games")}
                  </span>
                </div>
                <div className="flex items-end justify-between gap-2">
                  <div className="min-w-0">
                    <p className="truncate text-lead font-semibold text-ink transition-colors group-hover:text-signal-text">
                      {collection.name}
                    </p>
                    <p className="mt-1 truncate text-meta text-ink-3">
                      {collection.owned ? t("Yours") : collection.owner.displayName ?? collection.owner.username}
                    </p>
                  </div>
                  <ChevronRight
                    className="rtl:-scale-x-100 size-4 shrink-0 text-ink-3 transition-transform group-hover:translate-x-0.5 group-hover:text-ink"
                    aria-hidden="true"
                  />
                </div>
              </Link>
            ))}
          </div>
        </section>
      )}

      <section className="mt-11" aria-labelledby="home-all">
        <SectionHeader
          id="home-all"
          title={t("All games")}
          count={games.length}
          actions={
            <Link to="/library" className="text-label text-ink-3 underline-offset-4 hover:text-ink hover:underline">
              {t("Open the library")}
            </Link>
          }
        />
        <div className="grid grid-cols-2 gap-x-5 gap-y-7 sm:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5 2xl:grid-cols-6">
          {games.slice(0, 12).map((game) => (
            <GameCard
              key={game.id}
              game={game}
              index={game.id}
              installed={installed.get(game.id)}
              update={updates.get(game.id)}
              onBookmark={() => bookmark.mutate({ id: game.id, bookmarked: !game.bookmarked })}
              onPlay={
                isNative() && installed.has(game.id)
                  ? () =>
                      launch.mutate({
                        id: game.id,
                        title: game.title,
                        executable: installed.get(game.id)?.executable,
                      })
                  : undefined
              }
              onUpdate={
                isNative() && updates.has(game.id)
                  ? () => updateGame.mutate({ id: game.id, title: game.title })
                  : undefined
              }
            />
          ))}
        </div>
      </section>

      {version && heroDetail.data && (
        <InstallDialog
          open={installOpen}
          onOpenChange={setInstallOpen}
          game={heroDetail.data}
          version={version}
        />
      )}
    </div>
  );
}
