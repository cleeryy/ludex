import { useEffect, useMemo, useState } from "react";
import { useLocation, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  ArrowLeft,
  Bookmark,
  BookmarkCheck,
  Clock,
  Download,
  FileSignature,
  FolderInput,
  FolderOpen,
  Cast,
  Link2,
  ListPlus,
  MonitorUp,
  MoreHorizontal,
  Pencil,
  Palette,
  Play,
  Power,
  RefreshCw,
  Search,
  Share2,
  Trash2,
} from "lucide-react";
import { api, assetUrl, USING_MOCKS } from "@/lib/api";
import { ApiError, errorMessage } from "@/lib/http";
import { parseServerGameId } from "@/lib/game-route";
import {
  availableGameSections,
  gameSectionDescription,
  gameSectionLabel,
  gameSectionPath,
  resolveGameSection,
  type GameSectionId,
} from "@/lib/game-sections";
import { cn } from "@/lib/cn";
import { formatBytes, formatEta, formatMinutes, formatPercent, formatRating, formatRelative, formatSpeed, formatYear, stateLabel } from "@/lib/format";
import { isNative, native } from "@/lib/native";
import type { GameVersion, ProgressState } from "@/lib/types";
import { useBookmark, useForceCloseGame, useGamePlaytime, useGameSession, useInstalledGames, useLaunchGame, useResumeGame, useUpdateGame, useUpdateMap } from "@/lib/hooks";
import { heroAction, nextForceClosePhase, sessionForGame, type ForceClosePhase } from "@/lib/game-session";
import { isRollbackInstall } from "@/lib/game-versions";
import { useDownloads } from "@/state/downloads";
import { AchievementsPanel } from "@/components/game/achievements";
import { RomPanel } from "@/components/game/rom-panel";
import { ControllerPanel } from "@/components/settings/controller-panel";
import { CloudSaves } from "@/components/game/cloud-saves";
import { ArtworkDialog } from "@/components/game/artwork-dialog";
import { MatchMetadataDialog } from "@/components/game/match-metadata";
import { InstallDialog } from "@/components/game/install-dialog";
import { RemoteInstallAction } from "@/components/game/remote-install";
import { InstallFiles } from "@/components/game/install-files";
import { GameSettingsPanel } from "@/components/game/game-settings";
import { VersionManager } from "@/components/game/version-manager";
import { AdoptInstallDialog } from "@/components/game/adopt-install";
import { DeleteAdoptedDialog } from "@/components/game/delete-adopted";
import { InstallerPanel } from "@/components/game/installer-panel";
import { LaunchOptionsPanel } from "@/components/game/launch-options";
import { MakeItRunPanel } from "@/components/game/make-it-run";
import { ProtonDbBadge } from "@/components/game/protondb-badge";
import { MoveInstallDialog } from "@/components/game/move-install";
import { MediaGallery } from "@/components/game/media-gallery";
import { GameNav } from "@/components/game/game-nav";
import { GameOverview } from "@/components/game/game-overview";
import { StartupCommandPanel } from "@/components/game/startup-command";
import { AddToCollectionDialog } from "@/components/collections/add-to-collection";
import { ShareDialog } from "@/components/share/share-dialog";
import { StreamDialog } from "@/components/game/stream-dialog";
import { SettingsSection } from "@/components/settings/settings-section";
import { useAuth, useIsEditor } from "@/state/auth";
import { useToast } from "@/state/toasts";
import { ThemeScope } from "@/components/theme/theme-scope";
import { GameBackdrop, GameCover } from "@/components/art/cover";
import { artFromUrl, hashString } from "@/lib/art";
import { STATE_TONE } from "@/components/game/state";
import { ErrorState } from "@/components/page";
import { NotFoundScreen } from "@/screens/not-found";
import { Badge, Button, IconButton, Progress, Skeleton } from "@/components/ui/controls";
import { ConfirmDialog } from "@/components/ui/dialog";
import { DropdownMenu, Select, useDropdown } from "@/components/ui/select";
import { t } from "@/lib/i18n";

const STATES: ProgressState[] = ["unplayed", "playing", "completed", "onHold", "abandoned", "planToPlay"];

/** Groups whose body is several panels; the group keeps the `h2`. */
const MULTI_PANEL: ReadonlySet<GameSectionId> = new Set(["overview", "files", "launch"]);

/** The visible panel heading for a single-panel group. */
function panelTitle(id: GameSectionId): string {
  switch (id) {
    case "rom":
      return t("ROM and emulator");
    case "settings":
      return t("Game settings");
    case "saves":
      return t("Cloud saves");
    case "achievements":
      return t("Achievements");
    default:
      return gameSectionLabel(id);
  }
}

export function GameDetailRoute() {
  const { id } = useParams();
  // Unmatched launcher imports use negative ids and never name a server game (#154).
  if (parseServerGameId(id) === null) return <NotFoundScreen />;
  return <GameDetailScreen />;
}

export function GameDetailScreen() {
  const { id } = useParams();
  const gameId = Number(id);
  const navigate = useNavigate();
  const location = useLocation();
  const [params] = useSearchParams();
  const isEditor = useIsEditor();
  const { user } = useAuth();
  const { push } = useToast();
  const queryClient = useQueryClient();
  const installedQuery = useInstalledGames();
  const installed = useMemo(
    () => new Map((installedQuery.data ?? []).map((entry) => [entry.gameId, entry])),
    [installedQuery.data],
  );
  const bookmark = useBookmark();
  const launch = useLaunchGame();
  const downloads = useDownloads();
  const updates = useUpdateMap();
  const updateGame = useUpdateGame();
  const [installOpen, setInstallOpen] = useState(false);
  const [installTarget, setInstallTarget] = useState<GameVersion | null>(null);
  const [adoptOpen, setAdoptOpen] = useState(false);
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [moveOpen, setMoveOpen] = useState(false);
  const [collectionOpen, setCollectionOpen] = useState(false);
  const [shareOpen, setShareOpen] = useState(false);
  const [artworkOpen, setArtworkOpen] = useState(false);
  const [matchOpen, setMatchOpen] = useState(false);
  const [streamOpen, setStreamOpen] = useState(false);

  const { data: game, isPending, isError, error, refetch } = useQuery({
    queryKey: ["game", gameId],
    queryFn: () => api.getGame(gameId),
    enabled: Number.isFinite(gameId),
  });

  // Desktop playtime for this game (#26): totals and recent sessions.
  const playtime = useGamePlaytime(gameId);
  // #191: the live session of this game, polled while the page is open.
  const sessionQuery = useGameSession(gameId);
  const running = sessionForGame(sessionQuery.data, gameId);
  const resume = useResumeGame();
  const forceClose = useForceCloseGame(gameId);
  const [forceClosePhase, setForceClosePhase] = useState<ForceClosePhase>("idle");
  // A game that exits (or crashes) dismisses the force-close dialog; a later
  // launch starts from `idle` again instead of a stale `closing`.
  const runningSessionId = running?.sessionId ?? null;
  useEffect(() => {
    if (!runningSessionId) {
      setForceClosePhase((phase) => nextForceClosePhase(phase, "done"));
    }
  }, [runningSessionId]);

  // #98: the Stream action only appears once Sunshine has the game.
  const streamTarget = useQuery({
    queryKey: ["stream-target", gameId],
    queryFn: () => native.streamTarget(gameId),
    enabled: isNative() && Number.isFinite(gameId),
    retry: false,
    staleTime: 30_000,
  });

  const [versionId, setVersionId] = useState<number | null>(null);
  const version = useMemo(() => {
    if (!game) return null;
    // The pinned version is the default for downloads, installs and updates
    // (#168); the archive picker in Files can override it for this page.
    return (
      game.versions.find((v) => v.id === versionId) ??
      game.versions.find((v) => v.id === game.defaultVersionId) ??
      game.versions[0] ??
      null
    );
  }, [game, versionId]);

  const desktop = isNative();
  const isInstalled = game ? installed.get(game.id) : undefined;
  const sections = useMemo(
    () =>
      availableGameSections({
        kind: game?.kind === "rom" ? "rom" : "game",
        native: desktop,
        installed: Boolean(installed.get(gameId)),
      }),
    [game?.kind, desktop, installed, gameId],
  );
  const availableIds = useMemo(() => sections.map((section) => section.id), [sections]);
  const requested = resolveGameSection(params.get("tab"), availableIds);
  const section = sections.find((entry) => entry.id === requested) ?? sections[0];
  // A deep link opens with the group heading in focus; a plain load does not
  // move the page at all.
  const [focusToken, setFocusToken] = useState(() => (params.get("tab") ? 1 : 0));

  // Normalise the address bar: an unknown group, a group this game/device does
  // not have (or one lost after an uninstall) lands on the group actually drawn.
  // Wait for the install list first: `?tab=settings` is valid as soon as the
  // desktop app has answered, not on the first paint.
  const installsSettled = !desktop || installedQuery.isSuccess || installedQuery.isError;
  useEffect(() => {
    if (!game || !section || !installsSettled) return;
    const expected = section.id === "overview" ? null : section.id;
    if (params.get("tab") !== expected) {
      navigate(gameSectionPath(game.id, section.id), { replace: true });
    }
  }, [game, installsSettled, navigate, params, section]);

  const back = () => {
    if (location.key === "default") navigate("/library");
    else navigate(-1);
  };

  if (isPending) return <DetailSkeleton />;

  if (isError) {
    const notFound = error instanceof ApiError && error.status === 404;
    return (
      <div className="page">
        <ErrorState
          title={notFound ? t("Not in the library") : t("This game didn’t load")}
          message={notFound ? t("That game is no longer indexed. It may have been removed from the games folder.") : errorMessage(error)}
          onRetry={() => void refetch()}
          action={
            <Button variant="ghost" onClick={back}>
              {t("Back")}
            </Button>
          }
        />
      </div>
    );
  }

  if (!game || !section) return null;

  const isAdopted = isInstalled?.source === "adopted";
  const update = updates.get(game.id);
  const activeJob = downloads.jobForGame(game.id);
  const resumableJob = downloads.resumableForGame(game.id);
  const installPct = activeJob?.bytesTotal ? Math.min(100, (activeJob.bytesDone / activeJob.bytesTotal) * 100) : null;
  const spec = artFromUrl(game.backgroundUrl, game.title);
  const dialogVersion = installTarget ?? version;
  const dialogRollback =
    dialogVersion && isInstalled
      ? isRollbackInstall(dialogVersion.id, isInstalled.versionId, game.versions)
      : false;

  const setProgress = async (state: ProgressState) => {
    try {
      await api.setProgress(game.id, state);
      void queryClient.invalidateQueries({ queryKey: ["game", game.id] });
      void queryClient.invalidateQueries({ queryKey: ["games"] });
      void queryClient.invalidateQueries({ queryKey: ["stats"] });
    } catch (cause) {
      push({ title: t("Progress not saved"), description: errorMessage(cause), tone: "danger" });
    }
  };

  const download = async (target?: GameVersion) => {
    if (USING_MOCKS) {
      push({ title: t("Demo build"), description: t("Downloads and installs need the real server. The browser flow is wired.") });
      return;
    }
    try {
      const ticket = await api.downloadTicket(game.id, (target ?? version)?.id);
      const url = assetUrl(ticket.url);
      if (!url) throw new Error(t("The server returned an empty download URL"));
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.rel = "noopener";
      anchor.click();
    } catch (cause) {
      push({ title: t("Download did not start"), description: errorMessage(cause), tone: "danger" });
    }
  };

  const requestInstall = () => {
    setInstallTarget(null);
    setInstallOpen(true);
  };

  const action = heroAction({
    running: Boolean(running),
    desktop,
    installed: Boolean(isInstalled),
    activeJobPhase: activeJob?.phase ?? null,
    hasActiveJob: Boolean(activeJob),
    hasResumableJob: Boolean(resumableJob),
  });
  // Same rule as the pre-#191 hero: a paused active job first, then a
  // resumable history entry.
  const resumeJob = activeJob?.phase === "paused" ? activeJob : resumableJob;

  const primary =
    action === "resume" ? (
      <Button
        variant="primary"
        size="lg"
        icon={<MonitorUp className="size-4" />}
        loading={resume.isPending}
        onClick={() => resume.mutate(game.id)}
      >
        {t("Resume")}
      </Button>
    ) : action === "play" ? (
      <Button
        variant="primary"
        size="lg"
        icon={<Play className="size-4" />}
        onClick={() => launch.mutate({ id: game.id, title: game.title, executable: isInstalled?.executable ?? game.launch.executable })}
      >
        {t("Play")}
      </Button>
    ) : action === "resumeInstall" ? (
      <Button
        variant="default"
        size="lg"
        icon={<Download className="size-4" />}
        onClick={() =>
          void downloads.resume(resumeJob!, {
            title: game.title,
            version: version?.version,
            executable: game.launch.executable,
          })
        }
      >
        {t("Resume install")}
      </Button>
    ) : action === "installing" ? (
      <Button variant="default" size="lg" icon={<Download className="size-4" />} onClick={() => navigate("/downloads")}>
        {t("Installing {percent}", { percent: installPct !== null ? formatPercent(installPct) : "…" })}
      </Button>
    ) : desktop ? (
      <Button variant="primary" size="lg" icon={<Download className="size-4" />} onClick={requestInstall}>
        {t("Install")}
      </Button>
    ) : (
      <Button variant="primary" size="lg" icon={<Download className="size-4" />} onClick={() => void download()}>
        {t("Download")}
      </Button>
    );

  const removeFromLudex = () => {
    void native
      .unlinkInstallation(game.id)
      .then(() => {
        void queryClient.invalidateQueries({ queryKey: ["installed"] });
        void queryClient.invalidateQueries({ queryKey: ["recent-games"] });
        void queryClient.invalidateQueries({ queryKey: ["game", game.id] });
        push({
          title: t("{title} removed from Ludex", { title: game.title }),
          description: t("The files were left where they are."),
        });
      })
      .catch((cause: Error) => push({ title: t("Could not remove the game"), description: cause.message, tone: "danger" }));
  };

  const menu: { label: string; icon?: React.ReactNode; danger?: boolean; onSelect: () => void }[] = [
    ...(isEditor
      ? [
          {
            label: t("Match metadata…"),
            icon: <Search className="size-3.5" />,
            onSelect: () => setMatchOpen(true),
          },
          {
            label: t("Refresh metadata"),
            icon: <RefreshCw className="size-3.5" />,
            onSelect: () => {
              void api
                .refreshMetadata(game.id)
                .then(() => push({ title: t("Metadata queued"), description: t("The server will fetch a fresh copy in the background."), tone: "success" }))
                .catch((cause: Error) => push({ title: t("Could not queue metadata"), description: cause.message, tone: "danger" }));
            },
          },
          ...(game.kind !== "rom"
            ? [
                {
                  label: t("Edit startup command"),
                  icon: <Pencil className="size-3.5" />,
                  onSelect: () => navigate(gameSectionPath(game.id, "launch")),
                },
              ]
            : []),
        ]
      : []),
    {
      label: t("Add to collection"),
      icon: <ListPlus className="size-3.5" />,
      onSelect: () => setCollectionOpen(true),
    },
    ...(isEditor
      ? [
          {
            label: t("Artwork"),
            icon: <Palette className="size-3.5" />,
            onSelect: () => setArtworkOpen(true),
          },
          {
            label: t("Share"),
            icon: <Share2 className="size-3.5" />,
            onSelect: () => setShareOpen(true),
          },
        ]
      : []),
    ...(desktop && !isInstalled && !activeJob
      ? [
          {
            label: t("Link existing installation"),
            icon: <Link2 className="size-3.5" />,
            onSelect: () => setAdoptOpen(true),
          },
        ]
      : []),
    ...(isInstalled
      ? [
          {
            label: t("Open install folder"),
            icon: <FolderOpen className="size-3.5" />,
            onSelect: () => void native.openPath(isInstalled.installPath).catch((cause: Error) => push({ title: t("Could not open the folder"), description: cause.message, tone: "danger" })),
          },
          // Moving an adopted folder would re-parent the user's own folder; the
          // adopted actions below cover the supported flows (#129).
          ...(isAdopted
            ? []
            : [
                {
                  label: t("Move install…"),
                  icon: <FolderInput className="size-3.5" />,
                  onSelect: () => setMoveOpen(true),
                },
              ]),
          ...(desktop
            ? [
                {
                  // The only Build manifest lives in Installed files; the menu
                  // points there instead of repeating the action (#189).
                  label: t("Installed files"),
                  icon: <FileSignature className="size-3.5" />,
                  onSelect: () => navigate(gameSectionPath(game.id, "files")),
                },
              ]
            : []),
          ...(isAdopted
            ? [
                {
                  label: t("Remove from Ludex"),
                  icon: <Link2 className="size-3.5" />,
                  onSelect: removeFromLudex,
                },
                {
                  label: t("Uninstall and delete files…"),
                  icon: <Trash2 className="size-3.5" />,
                  danger: true,
                  onSelect: () => setDeleteOpen(true),
                },
              ]
            : [
                {
                  // Launcher imports (#116) own their files: the row goes, the
                  // game stays installed by its own client.
                  label: isInstalled.source && isInstalled.source !== "ludex" ? t("Remove from Ludex") : t("Uninstall"),
                  icon: <Trash2 className="size-3.5" />,
                  danger: true,
                  onSelect: () => {
                    void native
                      .uninstallGame(game.id)
                      .then(() => {
                        void queryClient.invalidateQueries({ queryKey: ["installed"] });
                        push({
                          title: t("Game removed"),
                          description: isInstalled.source && isInstalled.source !== "ludex"
                            ? t("The launcher entry was removed. Its files were not touched.")
                            : t("{title} was uninstalled from this machine.", { title: game.title }),
                        });
                      })
                      .catch((cause: Error) => push({ title: t("Could not uninstall"), description: cause.message, tone: "danger" }));
                  },
                },
              ]),
        ]
      : []),
  ];

  const versionPicker =
    game.versions.length > 1 && version ? (
      <div className="flex flex-wrap items-center gap-3">
        <span className="text-[12px] text-ink-3">{t("Game version")}</span>
        <div className="w-full sm:w-[240px]">
          <Select
            label={t("Game version")}
            value={version.id}
            onChange={setVersionId}
            options={game.versions.map((entry) => ({
              value: entry.id,
              label: entry.version ? `v${entry.version}` : entry.fileName,
              hint: formatBytes(entry.size),
            }))}
            className="h-10 text-[13px]"
          />
        </div>
      </div>
    ) : undefined;

  const body = (id: GameSectionId) => {
    switch (id) {
      case "overview":
        return (
          <GameOverview
            game={game}
            installed={isInstalled}
            session={running}
            update={update}
            preferredVersion={version}
            playtime={playtime.data ?? undefined}
            onInstall={requestInstall}
            onDownload={() => void download()}
            onLinkExisting={() => setAdoptOpen(true)}
            remoteInstall={
              !desktop && user && !isInstalled ? (
                <RemoteInstallAction gameId={game.id} versionId={version?.id} title={game.title} />
              ) : undefined
            }
          />
        );
      case "rom":
        return <RomPanel game={game} />;
      case "files":
        return (
          <div className="flex flex-col gap-8">
            <VersionManager
              game={game}
              installed={isInstalled}
              isEditor={isEditor}
              onDownload={(entry) => void download(entry)}
              onInstall={(entry) => {
                setInstallTarget(entry);
                setInstallOpen(true);
              }}
              selection={versionPicker}
            />
            {isInstalled && desktop && <InstallFiles gameId={game.id} title={game.title} />}
            {isInstalled && desktop && (
              <InstallerPanel gameId={game.id} title={game.title} executable={isInstalled.executable} />
            )}
          </div>
        );
      case "launch":
        return (
          <div className="flex flex-col gap-8">
            {isEditor && game.kind !== "rom" && <StartupCommandPanel game={game} installed={isInstalled} />}
            {isInstalled && desktop && game.kind !== "rom" && <MakeItRunPanel gameId={game.id} />}
            {isInstalled && desktop && game.kind !== "rom" && <LaunchOptionsPanel gameId={game.id} title={game.title} />}
            <ControllerPanel gameId={game.id} gameTitle={game.title} plain />
          </div>
        );
      case "settings":
        return <GameSettingsPanel gameId={game.id} title={game.title} />;
      case "saves":
        return <CloudSaves gameId={game.id} title={game.title} installed={Boolean(isInstalled)} />;
      case "achievements":
        return <AchievementsPanel gameId={game.id} isEditor={isEditor} installPath={isInstalled?.installPath} />;
    }
  };

  return (
    <div className="relative">
      <ThemeScope as="section" className="relative isolate overflow-hidden bg-stage">
        <GameBackdrop title={game.title} backgroundUrl={game.backgroundUrl} className="absolute inset-0 h-full" />

        <div className="page relative pb-16 pt-5">
        <button
          type="button"
          onClick={back}
          className="inline-flex h-8 items-center gap-2 text-[12.5px] text-ink-2 transition-colors hover:text-ink"
        >
          <ArrowLeft className="rtl:-scale-x-100 size-3.5" aria-hidden="true" />
          {t("Back")}
        </button>

        <div className="mt-[16vh] grid gap-8 lg:mt-[13vh] lg:grid-cols-[250px_1fr] lg:gap-12 xl:grid-cols-[280px_1fr]">
          <div className="w-[150px] sm:w-[190px] lg:w-full">
            <div className="shadow-plate">
              <GameCover title={game.title} coverUrl={game.coverUrl} index={game.id} year={formatYear(game.releaseDate)} genre={game.genres[0]} />
            </div>
          </div>

          <div className="min-w-0">
            <div className="flex flex-wrap items-center gap-x-3 gap-y-2 font-mono text-[11px] uppercase tracking-[0.08em] text-ink-3">
              <span>{formatYear(game.releaseDate)}</span>
              {game.ageRating && (
                <>
                  <span className="h-3 w-px bg-line-2" aria-hidden="true" />
                  <span>{game.ageRating}+</span>
                </>
              )}
              <span className="h-3 w-px bg-line-2" aria-hidden="true" />
              <span>{game.genres.join(" / ")}</span>
              <ProtonDbBadge gameId={game.id} />
              {game.earlyAccess && <Badge tone="signal">{t("Early access")}</Badge>}
              {running && <Badge tone="ok">{t("Running")}</Badge>}
              {game.kind === "rom" && <Badge tone="outline">{t("ROM")}</Badge>}
            </div>

            <h1 className="mt-3 max-w-[18ch] text-[clamp(2.4rem,4.6vw,4.25rem)] font-semibold leading-[0.94] tracking-[-0.035em]">
              {game.title}
            </h1>

            <div className="mt-5 flex flex-wrap items-center gap-x-5 gap-y-2 text-[12.5px] text-ink-2">
              {formatRating(game.rating) && (
                <span className="flex items-baseline gap-1.5">
                  <span className="num text-[15px] text-ink">{formatRating(game.rating)}</span>
                  <span className="text-[11px] text-ink-3">/ 100</span>
                </span>
              )}
              <span className="num">{formatBytes(game.totalSize)}</span>
              <span>{game.versionCount === 1 ? t("1 version") : `${game.versionCount} versions`}</span>
              {formatMinutes(game.minutesPlayed) && (
                <span className="flex items-center gap-1.5">
                  <Clock className="size-3.5 text-ink-3" aria-hidden="true" />
                  <span className="num">{formatMinutes(game.minutesPlayed)}</span>
                </span>
              )}
              {game.lastPlayedAt && <span className="text-ink-3">{t("Last played")} {formatRelative(game.lastPlayedAt)}</span>}
            </div>

            {game.description && <p className="measure mt-6 text-[14.5px] leading-relaxed text-ink-2">{game.description}</p>}

            <div className="mt-7 flex flex-wrap items-center gap-2">
              {primary}
              {running && (
                <Button
                  variant="danger"
                  size="lg"
                  icon={<Power className="size-4" />}
                  onClick={() => setForceClosePhase((phase) => nextForceClosePhase(phase, "ask"))}
                >
                  {t("Force close")}
                </Button>
              )}
              {update && isInstalled && !activeJob && (
                <Button
                  variant="default"
                  size="lg"
                  icon={<RefreshCw className="size-4" />}
                  loading={updateGame.isPending}
                  onClick={() => updateGame.mutate({ id: game.id, title: game.title })}
                >
                  {update.latestVersionLabel ? `Update to v${update.latestVersionLabel}` : t("Update")}
                </Button>
              )}
              {streamTarget.data?.published && (
                <Button variant="default" size="lg" icon={<Cast className="size-4" />} onClick={() => setStreamOpen(true)}>
                  {t("Stream")}
                </Button>
              )}
              <IconButton
                label={game.bookmarked ? t("Remove bookmark") : t("Bookmark")}
                variant="default"
                aria-pressed={game.bookmarked}
                className={cn("size-12", game.bookmarked && "border-signal/60 text-signal-text")}
                onClick={() => bookmark.mutate({ id: game.id, bookmarked: !game.bookmarked })}
              >
                {game.bookmarked ? <BookmarkCheck className="size-4" /> : <Bookmark className="size-4" />}
              </IconButton>
              <DetailMenu items={menu} />
            </div>

            {activeJob && (
              <div className="mt-4 max-w-[360px]">
                <Progress value={installPct} label={t("Install progress")} />
                <p className="mt-1.5 flex flex-wrap items-baseline gap-x-2.5 font-mono text-[10.5px] text-ink-3">
                  <span className="text-ink-2">
                    {activeJob.phase === "downloading" ? t("Downloading") : activeJob.phase === "paused" ? t("Paused") : activeJob.phase}
                  </span>
                  {installPct !== null && <span>{formatPercent(installPct)}</span>}
                  {activeJob.speedBps > 0 && <span>{formatSpeed(activeJob.speedBps)}</span>}
                  {activeJob.bytesTotal && activeJob.speedBps > 0 && (
                    <span>{t("{duration} left", { duration: formatEta(activeJob.bytesDone, activeJob.bytesTotal, activeJob.speedBps) ?? "" })}</span>
                  )}
                </p>
              </div>
            )}

            <div className="mt-5 flex flex-wrap items-center gap-3">
              <StatePicker state={game.state} onChange={setProgress} />
            </div>
          </div>
        </div>
        </div>
      </ThemeScope>
      <div className="relative z-10 -mt-16 h-16 bg-gradient-to-b from-transparent to-bg" aria-hidden="true" />

      <div className="page pt-8">
        <MediaGallery game={game} spec={spec} seed={hashString(game.title)} />

        <div className="mx-auto mt-10 flex w-full max-w-[1240px] flex-col gap-8 lg:flex-row lg:items-start lg:gap-12">
          <GameNav
            gameId={game.id}
            sections={sections}
            active={section.id}
            onSelect={() => setFocusToken((token) => token + 1)}
          />
          <div className="min-w-0 flex-1">
            <SettingsSection
              id={section.id}
              title={MULTI_PANEL.has(section.id) ? gameSectionLabel(section.id) : panelTitle(section.id)}
              description={gameSectionDescription(section.id)}
              heading={MULTI_PANEL.has(section.id) ? "section" : "panel"}
              focusToken={focusToken}
              scroll="self"
              headingPrefix="game"
              className="scroll-mt-16 lg:scroll-mt-0"
            >
              {body(section.id)}
            </SettingsSection>
          </div>
        </div>
      </div>

      {dialogVersion && (
        <InstallDialog
          open={installOpen}
          onOpenChange={setInstallOpen}
          game={game}
          version={dialogVersion}
          rollback={dialogRollback}
        />
      )}

      <AdoptInstallDialog
        open={adoptOpen}
        onOpenChange={setAdoptOpen}
        gameId={game.id}
        title={game.title}
        versions={game.versions.map((entry) => ({
          id: entry.id,
          version: entry.version,
          fileName: entry.fileName,
        }))}
      />

      {isAdopted && (
        <DeleteAdoptedDialog
          open={deleteOpen}
          onOpenChange={setDeleteOpen}
          gameId={game.id}
          title={game.title}
        />
      )}

      {isInstalled && !isAdopted && (
        <MoveInstallDialog
          open={moveOpen}
          onOpenChange={setMoveOpen}
          gameId={game.id}
          title={game.title}
          installed={isInstalled}
          onMoved={() => {
            void queryClient.invalidateQueries({ queryKey: ["installed"] });
            void queryClient.invalidateQueries({ queryKey: ["game", game.id] });
            void queryClient.invalidateQueries({ queryKey: ["game-executables"] });
          }}
        />
      )}
      <AddToCollectionDialog
        open={collectionOpen}
        onOpenChange={setCollectionOpen}
        gameId={game.id}
        gameTitle={game.title}
      />
      <ShareDialog open={shareOpen} onOpenChange={setShareOpen} game={game} />
      <StreamDialog gameId={game.id} title={game.title} open={streamOpen} onOpenChange={setStreamOpen} />
      <ArtworkDialog open={artworkOpen} onOpenChange={setArtworkOpen} game={game} />
      <MatchMetadataDialog open={matchOpen} onOpenChange={setMatchOpen} game={game} />

      {/* #191: the destructive path always asks first and spells out how it
          differs from the normal stop. */}
      {running && (
        <ConfirmDialog
          open={forceClosePhase !== "idle"}
          onOpenChange={(open) =>
            setForceClosePhase((phase) => nextForceClosePhase(phase, open ? "ask" : "cancel"))
          }
          title={t("Force close {title}?", { title: game.title })}
          description={t("Use this when the game will not exit. Unsaved progress may be lost: Ludex ends the game's whole process tree.")}
          confirmLabel={t("Force close")}
          loading={forceClosePhase === "closing"}
          onConfirm={() => {
            setForceClosePhase((phase) => nextForceClosePhase(phase, "confirm"));
            forceClose.mutate(running.sessionId, {
              onSettled: () => setForceClosePhase((phase) => nextForceClosePhase(phase, "done")),
            });
          }}
        />
      )}
    </div>
  );
}

/* --------------------------------- pieces -------------------------------- */

function DetailMenu({ items }: { items: { label: string; icon?: React.ReactNode; danger?: boolean; onSelect: () => void }[] }) {
  const menu = useDropdown();
  return (
    <>
      <IconButton {...menu.triggerProps} label={t("More actions")} variant="default" className="size-12">
        <MoreHorizontal className="size-4" />
      </IconButton>
      <DropdownMenu {...menu.menuProps} items={items} label={t("Game actions")} />
    </>
  );
}

function StatePicker({ state, onChange }: { state: ProgressState; onChange: (state: ProgressState) => void }) {
  const menu = useDropdown();
  return (
    <>
      <button
        {...menu.triggerProps}
        className="flex h-9 items-center gap-2 rounded-[2px] border border-line bg-surface px-3 text-[12.5px] text-ink-2 transition-colors hover:border-line-2 hover:text-ink"
      >
        <span className={cn("size-1.5 rounded-full", STATE_TONE[state].dot)} aria-hidden="true" />
        <span className="text-ink-3">{t("Progress")}</span>
        <span className="text-ink">{stateLabel(state)}</span>
      </button>
      <DropdownMenu
        {...menu.menuProps}
        label={t("Set progress state")}
        align="start"
        items={STATES.map((value) => ({
          label: stateLabel(value),
          selected: value === state,
          onSelect: () => onChange(value),
        }))}
      />
    </>
  );
}

function DetailSkeleton() {
  return (
    <div className="page">
      <Skeleton className="h-3 w-16" />
      <div className="mt-[18vh] grid gap-8 lg:grid-cols-[250px_1fr] lg:gap-12">
        <Skeleton className="aspect-[3/4] w-[150px] sm:w-[190px] lg:w-full" />
        <div className="flex flex-col gap-4">
          <Skeleton className="h-3 w-40" />
          <Skeleton className="h-12 w-3/4" />
          <Skeleton className="h-3 w-56" />
          <Skeleton className="mt-4 h-3 w-full max-w-[520px]" />
          <Skeleton className="h-3 w-full max-w-[460px]" />
          <Skeleton className="mt-4 h-12 w-40" />
        </div>
      </div>
    </div>
  );
}
