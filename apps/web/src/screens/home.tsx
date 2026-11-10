import { useMemo, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { Download, Play } from "lucide-react";
import { api } from "@/lib/api";
import { downloadGame } from "@/lib/download";
import { errorMessage } from "@/lib/http";
import { formatBytes, formatMinutes, formatRating, formatRelative, formatYear, stateLabel } from "@/lib/format";
import { isNative } from "@/lib/native";
import { installsAvailable } from "@/lib/install";
import { useAllGames, useBookmark, useInstalledMap, useLaunchGame } from "@/lib/hooks";
import { useIsEditor } from "@/state/auth";
import { useDownloads } from "@/state/downloads";
import { useToast } from "@/state/toasts";
import { ArtPlate } from "@/components/art/cover";
import { GameCard } from "@/components/game/game-card";
import { InstallDialog } from "@/components/game/install-dialog";
import { EmptyState, ErrorState, SectionHeader } from "@/components/page";
import { Button, Skeleton } from "@/components/ui/controls";

export function HomeScreen() {
  const navigate = useNavigate();
  const { push } = useToast();
  const isEditor = useIsEditor();
  const installed = useInstalledMap();
  const downloads = useDownloads();
  const launch = useLaunchGame();
  const bookmark = useBookmark();
  const [installOpen, setInstallOpen] = useState(false);

  const { data, isPending, isError, error, refetch } = useAllGames();
  const games = data?.items ?? [];

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
      push({ title: "Download did not start", description: errorMessage(cause), tone: "danger" });
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
          title="Home didn’t load"
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
          title="Nothing on the shelf yet"
          description={
            isEditor
              ? "The games folder is empty. Paste a download link and Ludex will fetch it, verify the archive and index it for you."
              : "This library is empty. An administrator can add games from the server, or drop archives into the games folder."
          }
        >
          {isEditor ? (
            <Button variant="primary" icon={<Download className="size-4" />} onClick={() => navigate("/admin/ingest")}>
              Add games
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
      Play
    </Button>
  ) : heroJob ? (
    <Button variant="default" size="lg" icon={<Download className="size-4" />} onClick={() => navigate("/downloads")}>
      Installing {heroPct !== null ? `${heroPct.toFixed(0)}%` : ""}
    </Button>
  ) : isNative() ? (
    <Button
      variant="primary"
      size="lg"
      icon={<Download className="size-4" />}
      disabled={!version}
      onClick={() => setInstallOpen(true)}
    >
      Install
    </Button>
  ) : (
    <Button variant="primary" size="lg" icon={<Download className="size-4" />} onClick={() => void download()}>
      Download
    </Button>
  );

  const playtime = formatMinutes(hero.minutesPlayed);
  const rating = formatRating(hero.rating);

  return (
    <div className="page page-in pt-6">
      <div className="mb-3 flex items-baseline justify-end gap-4 font-mono text-[10.5px] text-ink-3">
        <span>{games.length} games</span>
        {installsAvailable() && <span>{installed.size} installed</span>}
      </div>

      <section className="force-dark relative overflow-hidden rounded-[2px] border border-line-2">
        <ArtPlate
          title={hero.title}
          artUrl={heroDetail.data?.backgroundUrl ?? hero.backgroundUrl}
          className="absolute inset-0 h-full w-full"
        />
        <div
          className="absolute inset-0"
          style={{
            background:
              "linear-gradient(90deg, rgba(5,5,5,0.95) 0%, rgba(5,5,5,0.74) 48%, rgba(5,5,5,0.34) 100%)",
          }}
        />
        <div
          className="absolute inset-0"
          style={{ background: "linear-gradient(0deg, rgba(5,5,5,0.92) 0%, rgba(5,5,5,0) 58%)" }}
        />

        <div className="relative flex min-h-[360px] flex-col justify-end gap-3.5 p-6 sm:min-h-[400px] sm:p-8">
          <span className="flex items-center gap-2.5 font-mono text-[10px] uppercase tracking-[0.14em] text-[#ffc61a]">
            <span className="h-px w-7 bg-[#ffc61a]" aria-hidden="true" />
            {hero.lastPlayedAt ? "Continue playing" : "Start here"}
          </span>
          <h1 className="max-w-[20ch] text-[clamp(2.1rem,4vw,3.6rem)] font-semibold leading-[0.95] tracking-[-0.035em] text-[#ece8de]">
            {hero.title}
          </h1>
          <div className="flex flex-wrap items-center gap-x-4 gap-y-1 font-mono text-[10.5px] text-[#a8a396]">
            <span>{formatYear(hero.releaseDate)}</span>
            <span>{stateLabel(hero.state)}</span>
            {rating && <span>{rating} / 100</span>}
            {playtime && <span>{playtime} played</span>}
            {hero.lastPlayedAt && <span>Last played {formatRelative(hero.lastPlayedAt)}</span>}
            {heroInstalled && <span className="text-[#79c99f]">Installed</span>}
          </div>
          {heroDetail.data?.description && (
            <p className="line-clamp-2 max-w-[58ch] text-[13.5px] leading-relaxed text-[#c9c4b8]">
              {heroDetail.data.description}
            </p>
          )}
          <div className="mt-1 flex flex-wrap items-center gap-2">
            {primary}
            <Button variant="default" onClick={() => navigate(`/game/${hero.id}`)}>
              Game page
            </Button>
            {heroJob && (
              <span className="text-[11.5px] text-[#a8a396]">
                {heroJob.bytesTotal ? `${formatBytes(heroJob.bytesDone)} of ${formatBytes(heroJob.bytesTotal)}` : "Preparing"}
              </span>
            )}
          </div>
        </div>
      </section>

      {recent.length > 0 && (
        <section className="mt-12" aria-labelledby="home-recent">
          <SectionHeader
            title="Recently played"
            actions={
              <Link to="/recent" className="text-[12px] text-ink-3 underline-offset-4 hover:text-ink hover:underline">
                See all
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

      <section className="mt-12" aria-labelledby="home-all">
        <SectionHeader
          title="All games"
          count={games.length}
          actions={
            <Link to="/library" className="text-[12px] text-ink-3 underline-offset-4 hover:text-ink hover:underline">
              Open the library
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
              onBookmark={() => bookmark.mutate({ id: game.id, bookmarked: !game.bookmarked })}
              onPlay={
                isNative() && installed.has(game.id)
                  ? () => launch.mutate({ id: game.id, title: game.title, executable: installed.get(game.id)?.executable })
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
