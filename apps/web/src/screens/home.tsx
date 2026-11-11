import { useMemo, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { Download, Play } from "lucide-react";
import { api } from "@/lib/api";
import { downloadGame } from "@/lib/download";
import { errorMessage } from "@/lib/http";
import { formatBytes, formatMinutes, formatRating, formatRelative, formatYear, stateLabel, formatPercent } from "@/lib/format";
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
import { useDownloads } from "@/state/downloads";
import { useToast } from "@/state/toasts";
import { ArtPlate } from "@/components/art/cover";
import { ThemeScope } from "@/components/theme/theme-scope";
import { GameCard } from "@/components/game/game-card";
import { InstallDialog } from "@/components/game/install-dialog";
import { GettingStartedCard } from "@/components/onboarding/getting-started";
import { EmptyState, ErrorState, SectionHeader } from "@/components/page";
import { Button, Skeleton } from "@/components/ui/controls";
import { t, tn } from "@/lib/i18n";

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

export function HomeScreen() {
  const navigate = useNavigate();
  const { push } = useToast();
  const isEditor = useIsEditor();
  const installed = useInstalledMap();
  const downloads = useDownloads();
  const launch = useLaunchGame();
  const updates = useUpdateMap();
  const updateGame = useUpdateGame();
  const recentOnDevice = useRecentGames(6);
  const bookmark = useBookmark();
  const [installOpen, setInstallOpen] = useState(false);

  const { data, isPending, isError, error, refetch } = useAllGames();
  const games = data?.items ?? [];
  const collectionsQuery = useQuery({ queryKey: ["collections"], queryFn: () => api.listCollections() });
  const shelves = (collectionsQuery.data ?? []).slice(0, 4);

  const played = useMemo(
    () =>
      games
        .filter((game) => game.lastPlayedAt)
        .sort((a, b) => Date.parse(b.lastPlayedAt ?? "") - Date.parse(a.lastPlayedAt ?? "")),
    [games],
  );

  const hero = useMemo(
    () => played.find((game) => installed.has(game.id)) ?? played[0] ?? games[0],
    [played, installed, games],
  );

  const heroDetail = useQuery({
    queryKey: ["game", hero?.id],
    queryFn: () => api.getGame(hero!.id),
    enabled: Boolean(hero),
    staleTime: 60_000,
  });

  const version = heroDetail.data?.versions[0];
  const heroInstalled = hero ? installed.get(hero.id) : undefined;
  const heroJob = hero ? downloads.jobForGame(hero.id) : undefined;
  const heroPct = heroJob?.bytesTotal ? Math.min(100, (heroJob.bytesDone / heroJob.bytesTotal) * 100) : heroJob ? 0 : null;

  const recent = played.filter((game) => game.id !== hero?.id).slice(0, 8);

  const gamesById = useMemo(() => new Map(games.map((game) => [game.id, game])), [games]);
  const deviceGames = (recentOnDevice.data ?? []).filter((entry) => entry.gameId !== hero?.id);

  const installingPercent = (gameId: number): number | null => {
    const job = downloads.jobForGame(gameId);
    if (!job) return null;
    return job.bytesTotal ? (job.bytesDone / job.bytesTotal) * 100 : 0;
  };

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
        <Skeleton className="h-[380px] w-full" />
        <div className="mt-12 grid grid-cols-2 gap-x-4 gap-y-8 sm:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5">
          {Array.from({ length: 5 }).map((_, index) => (
            <div key={index}>
              <Skeleton className="aspect-[3/4] w-full" />
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
        launch.mutate({ id: hero.id, title: hero.title, executable: heroInstalled.executable ?? heroDetail.data?.launch.executable })
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

  return (
    <div className="page page-in pt-6">
      <div className="mb-3 flex items-baseline justify-end gap-4 font-mono text-[10.5px] text-ink-3">
        <span>{tn(games.length, "1 game", "{count} games")}</span>
        {installsAvailable() && <span>{t("{count} installed", { count: installed.size })}</span>}
      </div>

      <GettingStartedCard className="mb-6" />

      <ThemeScope as="section" className="relative overflow-hidden rounded-[2px] border border-line-2">
        <ArtPlate
          title={hero.title}
          artUrl={heroDetail.data?.backgroundUrl ?? hero.backgroundUrl}
          className="absolute inset-0 h-full w-full"
        />
        <div className="absolute inset-0 hero-shade" />

        <div className="relative flex min-h-[360px] flex-col justify-end gap-3.5 p-6 sm:min-h-[400px] sm:p-8">
          <span className="flex items-center gap-2.5 font-mono text-[10px] uppercase tracking-[0.14em] text-signal">
            <span className="h-px w-7 bg-signal" aria-hidden="true" />
            {hero.lastPlayedAt ? t("Continue playing") : t("Start here")}
          </span>
          <h1 className="max-w-[20ch] text-[clamp(2.1rem,4vw,3.6rem)] font-semibold leading-[0.95] tracking-[-0.035em] text-stage-ink">
            {hero.title}
          </h1>
          <div className="flex flex-wrap items-center gap-x-4 gap-y-1 font-mono text-[10.5px] text-stage-ink-2">
            <span>{formatYear(hero.releaseDate)}</span>
            <span>{stateLabel(hero.state)}</span>
            {rating && <span>{rating} / 100</span>}
            {playtime && <span>{t("{duration} played", { duration: playtime })}</span>}
            {hero.lastPlayedAt && <span>{t("Last played {when}", { when: formatRelative(hero.lastPlayedAt) })}</span>}
            {heroInstalled && <span className="text-stage-ok">{t("Installed")}</span>}
          </div>
          {heroDetail.data?.description && (
            <p className="line-clamp-2 max-w-[58ch] text-[13.5px] leading-relaxed text-stage-ink-2">
              {heroDetail.data.description}
            </p>
          )}
          <div className="mt-1 flex flex-wrap items-center gap-2">
            {primary}
            <Button variant="default" onClick={() => navigate(`/game/${hero.id}`)}>
              {t("Game page")}
            </Button>
            {heroJob && (
              <span className="text-[11.5px] text-stage-ink-2">
                {heroJob.bytesTotal ? `${formatBytes(heroJob.bytesDone)} of ${formatBytes(heroJob.bytesTotal)}` : t("Preparing")}
              </span>
            )}
          </div>
        </div>
      </ThemeScope>

      {deviceGames.length > 0 && (
        <section className="mt-12" aria-labelledby="home-device">
          <SectionHeader
            title={t("On this device")}
            description={t("Installed here, newest launch first.")}
            actions={
              <Link to="/downloads" className="text-[12px] text-ink-3 underline-offset-4 hover:text-ink hover:underline">
                {t("Manage installs")}
              </Link>
            }
          />
          <ul id="home-device" className="border-t border-line">
            {deviceGames.map((entry) => {
              const game = gamesById.get(entry.gameId);
              const update = updates.get(entry.gameId);
              return (
                <li key={entry.gameId} className="flex items-center gap-4 border-b border-line py-3">
                  <div className="min-w-0 flex-1">
                    {game ? (
                      <Link to={`/game/${entry.gameId}`} className="truncate text-[13.5px] font-medium text-ink hover:underline">
                        {entry.title}
                      </Link>
                    ) : (
                      <span className="truncate text-[13.5px] font-medium text-ink">{entry.title}</span>
                    )}
                    <p className="mt-0.5 flex flex-wrap items-baseline gap-x-2.5 font-mono text-[10.5px] text-ink-3">
                      {entry.version && <span>{`v${entry.version}`}</span>}
                      <span>{formatBytes(entry.sizeBytes)}</span>
                      {entry.source === "adopted" && (
                        <span className="border border-signal/40 px-1.5 py-[1px] text-[9.5px] uppercase tracking-[0.08em] text-signal-text">
                          {t("Adopted")}
                        </span>
                      )}
                      {entry.source && entry.source !== "ludex" && entry.source !== "adopted" && (
                        <span>{sourceLabel(entry.source)}</span>
                      )}
                      {entry.lastLaunchedAt && <span>{t("Last played")} {formatRelative(entry.lastLaunchedAt)}</span>}
                      {update && <span className="text-signal-text">{t("Update available")}</span>}
                    </p>
                  </div>
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
                    variant="default"
                    size="sm"
                    icon={<Play className="size-3.5" />}
                    onClick={() => launch.mutate({ id: entry.gameId, title: entry.title, executable: entry.executable })}
                  >
                    {t("Play")}
                  </Button>
                  {!game && (
                    <span className="font-mono text-[10px] text-ink-3">
                      {entry.source ? sourceLabel(entry.source) : t("Not in this library")}
                    </span>
                  )}
                </li>
              );
            })}
          </ul>
        </section>
      )}

      {recent.length > 0 && (
        <section className="mt-12" aria-labelledby="home-recent">
          <SectionHeader
            title={t("Recently played")}
            actions={
              <Link to="/recent" className="text-[12px] text-ink-3 underline-offset-4 hover:text-ink hover:underline">
                {t("See all")}
              </Link>
            }
          />
          <div id="home-recent" className="nav-scroll -mx-[var(--page-x)] flex gap-4 overflow-x-auto px-[var(--page-x)] pb-2">
            {recent.map((game) => (
              <div key={game.id} className="w-[148px] shrink-0">
                <GameCard
                  game={game}
                  index={game.id}
                  installed={installed.get(game.id)}
                  installing={installingPercent(game.id)}
                  onBookmark={() => bookmark.mutate({ id: game.id, bookmarked: !game.bookmarked })}
                  onPlay={
                    isNative() && installed.has(game.id)
                      ? () => launch.mutate({ id: game.id, title: game.title, executable: installed.get(game.id)?.executable })
                      : undefined
                  }
                />
              </div>
            ))}
          </div>
        </section>
      )}

      {shelves.length > 0 && (
        <section className="mt-12" aria-labelledby="home-collections">
          <SectionHeader
            title={t("Collections")}
            actions={
              <Link to="/collections" className="text-[12px] text-ink-3 underline-offset-4 hover:text-ink hover:underline">
                {t("All collections")}
              </Link>
            }
          />
          <div id="home-collections" className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
            {shelves.map((collection) => (
              <Link
                key={collection.id}
                to={`/collections/${collection.id}`}
                className="flex flex-col justify-between gap-4 border border-line bg-surface/40 p-3 transition-colors hover:border-line-2 hover:bg-surface"
              >
                <div className="flex items-center justify-between gap-2">
                  <span className="font-mono text-[9.5px] uppercase tracking-[0.12em] text-ink-3">
                    {collection.kind === "smart" ? t("Smart") : t("Manual")}
                  </span>
                  <span className="num text-[10.5px] text-ink-3">{collection.gameCount}</span>
                </div>
                <div>
                  <p className="truncate text-[14px] font-medium text-ink">{collection.name}</p>
                  <p className="mt-1 truncate font-mono text-[10px] text-ink-3">
                    {collection.owned ? t("Yours") : collection.owner.displayName ?? collection.owner.username}
                  </p>
                </div>
              </Link>
            ))}
          </div>
        </section>
      )}

      <section className="mt-12" aria-labelledby="home-all">
        <SectionHeader
          title={t("All games")}
          count={games.length}
          actions={
            <Link to="/library" className="text-[12px] text-ink-3 underline-offset-4 hover:text-ink hover:underline">
              {t("Open the library")}
            </Link>
          }
        />
        <div
          id="home-all"
          className="grid grid-cols-2 gap-x-4 gap-y-8 sm:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5"
        >
          {games.slice(0, 10).map((game) => (
            <GameCard
              key={game.id}
              game={game}
              index={game.id}
              installed={installed.get(game.id)}
              installing={installingPercent(game.id)}
              update={updates.get(game.id)}
              onBookmark={() => bookmark.mutate({ id: game.id, bookmarked: !game.bookmarked })}
              onPlay={
                isNative() && installed.has(game.id)
                  ? () => launch.mutate({ id: game.id, title: game.title, executable: installed.get(game.id)?.executable })
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

      {version && (
        <InstallDialog
          open={installOpen}
          onOpenChange={setInstallOpen}
          game={heroDetail.data!}
          version={version}
        />
      )}
    </div>
  );
}
