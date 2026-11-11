import { useMemo, useState } from "react";
import { useLocation, useNavigate, useParams } from "react-router-dom";
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
  MoreHorizontal,
  Pencil,
  Palette,
  Play,
  RefreshCw,
  Share2,
  Trash2,
} from "lucide-react";
import { api, assetUrl, USING_MOCKS } from "@/lib/api";
import { ApiError, errorMessage } from "@/lib/http";
import { cn } from "@/lib/cn";
import { formatBytes, formatDate, formatEta, formatMinutes, formatRating, formatRelative, formatSeconds, formatSpeed, formatYear, stateLabel, formatPercent } from "@/lib/format";
import { isNative, native } from "@/lib/native";
import type { GameDetail, InstalledGame, ProgressState } from "@/lib/types";
import { useBookmark, useGamePlaytime, useInstalledMap, useLaunchGame, useUpdateGame, useUpdateMap } from "@/lib/hooks";
import { useDownloads } from "@/state/downloads";
import { AchievementsPanel } from "@/components/game/achievements";
import { RomPanel } from "@/components/game/rom-panel";
import { ControllerPanel } from "@/components/settings/controller-panel";
import { CloudSaves } from "@/components/game/cloud-saves";
import { ArtworkDialog } from "@/components/game/artwork-dialog";
import { InstallDialog } from "@/components/game/install-dialog";
import { InstallFiles } from "@/components/game/install-files";
import { GameSettingsPanel } from "@/components/game/game-settings";
import { AdoptInstallDialog } from "@/components/game/adopt-install";
import { DeleteAdoptedDialog } from "@/components/game/delete-adopted";
import { InstallerPanel } from "@/components/game/installer-panel";
import { ExecutableField } from "@/components/game/executable-picker";
import { LaunchOptionsPanel } from "@/components/game/launch-options";
import { ProtonDbBadge } from "@/components/game/protondb-badge";
import { MoveInstallDialog } from "@/components/game/move-install";
import { MediaGallery } from "@/components/game/media-gallery";
import { AddToCollectionDialog } from "@/components/collections/add-to-collection";
import { ShareDialog } from "@/components/share/share-dialog";
import { StreamDialog } from "@/components/game/stream-dialog";
import { useAuth, useIsEditor } from "@/state/auth";
import { useToast } from "@/state/toasts";
import { GameBackdrop, GameCover } from "@/components/art/cover";
import { artFromUrl, hashString } from "@/lib/art";
import { STATE_TONE } from "@/components/game/state";
import { ErrorState, SectionHeader } from "@/components/page";
import { Badge, Button, IconButton, Input, Progress, Skeleton } from "@/components/ui/controls";
import { DropdownMenu, Select, useDropdown } from "@/components/ui/select";
import { t } from "@/lib/i18n";

const STATES: ProgressState[] = ["unplayed", "playing", "completed", "onHold", "abandoned", "planToPlay"];

export function GameDetailScreen() {
  const { id } = useParams();
  const gameId = Number(id);
  const navigate = useNavigate();
  const location = useLocation();
  const isEditor = useIsEditor();
  const { user } = useAuth();
  const { push } = useToast();
  const queryClient = useQueryClient();
  const installed = useInstalledMap();
  const bookmark = useBookmark();
  const launch = useLaunchGame();
  const downloads = useDownloads();
  const updates = useUpdateMap();
  const updateGame = useUpdateGame();
  const [installOpen, setInstallOpen] = useState(false);
  const [adoptOpen, setAdoptOpen] = useState(false);
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [moveOpen, setMoveOpen] = useState(false);
  const [collectionOpen, setCollectionOpen] = useState(false);
  const [shareOpen, setShareOpen] = useState(false);
  const [artworkOpen, setArtworkOpen] = useState(false);
  const [launchOpen, setLaunchOpen] = useState(false);
  const [streamOpen, setStreamOpen] = useState(false);

  const { data: game, isPending, isError, error, refetch } = useQuery({
    queryKey: ["game", gameId],
    queryFn: () => api.getGame(gameId),
    enabled: Number.isFinite(gameId),
  });

  // Desktop playtime for this game (#26): totals and recent sessions.
  const playtime = useGamePlaytime(gameId);

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
    return game.versions.find((v) => v.id === versionId) ?? game.versions[0] ?? null;
  }, [game, versionId]);

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

  if (!game) return null;

  const isInstalled = installed.get(game.id);
  const isAdopted = isInstalled?.source === "adopted";
  const update = updates.get(game.id);
  const activeJob = downloads.jobForGame(game.id);
  const resumableJob = downloads.resumableForGame(game.id);
  const installPct = activeJob?.bytesTotal ? Math.min(100, (activeJob.bytesDone / activeJob.bytesTotal) * 100) : null;
  const canPlay = isNative() && Boolean(isInstalled);
  const spec = artFromUrl(game.backgroundUrl, game.title);
  const totalPlayed = formatSeconds(playtime.data?.totalSeconds) ?? formatMinutes(game.minutesPlayed);

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

  const download = async () => {
    if (USING_MOCKS) {
      push({ title: t("Demo build"), description: t("Downloads and installs need the real server. The browser flow is wired.") });
      return;
    }
    try {
      const ticket = await api.downloadTicket(game.id, version?.id);
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

  const primary = canPlay ? (
    <Button
      variant="primary"
      size="lg"
      icon={<Play className="size-4" />}
      onClick={() => launch.mutate({ id: game.id, title: game.title, executable: isInstalled?.executable ?? game.launch.executable })}
    >
      {t("Play")}
    </Button>
  ) : activeJob?.phase === "paused" ? (
    <Button
      variant="default"
      size="lg"
      icon={<Download className="size-4" />}
      onClick={() =>
        void downloads.resume(activeJob, {
          title: game.title,
          version: version?.version,
          executable: game.launch.executable,
        })
      }
    >
      {t("Resume install")}
    </Button>
  ) : activeJob ? (
    <Button variant="default" size="lg" icon={<Download className="size-4" />} onClick={() => navigate("/downloads")}>
      {t("Installing {percent}", { percent: installPct !== null ? formatPercent(installPct) : "…" })}
    </Button>
  ) : resumableJob ? (
    <Button
      variant="default"
      size="lg"
      icon={<Download className="size-4" />}
      onClick={() =>
        void downloads.resume(resumableJob, {
          title: game.title,
          version: version?.version,
          executable: game.launch.executable,
        })
      }
    >
      {t("Resume install")}
    </Button>
  ) : isNative() ? (
    <Button variant="primary" size="lg" icon={<Download className="size-4" />} onClick={() => setInstallOpen(true)}>
      {t("Install")}
    </Button>
  ) : (
    <Button variant="primary" size="lg" icon={<Download className="size-4" />} onClick={() => void download()}>
      {t("Download")}
    </Button>
  );

  const buildManifest = () => {
    void native
      .buildInstallManifest(game.id)
      .then((report) =>
        push({
          title: t("Manifest built for {title}", { title: game.title }),
          description: t("{files} hashed ({size}). Verify can now compare the folder with it.", {
            files: report.files === 1 ? t("1 file") : t("{count} files", { count: report.files }),
            size: formatBytes(report.bytes),
          }),
          tone: "success",
        }),
      )
      .catch((cause: Error) =>
        push({ title: t("Could not build the manifest"), description: cause.message, tone: "danger" }),
      );
  };

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

  const menu = [
    ...(isEditor
      ? [
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
          {
            label: t("Edit launch options"),
            icon: <Pencil className="size-3.5" />,
            onSelect: () => {
              setLaunchOpen(true);
              requestAnimationFrame(() => document.getElementById("launch-options")?.scrollIntoView({ behavior: "smooth", block: "start" }));
            },
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
          ...(isNative()
            ? [
                {
                  label: t("Build manifest"),
                  icon: <FileSignature className="size-3.5" />,
                  onSelect: buildManifest,
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

  return (
    <div className="relative">
      <section className="force-dark relative isolate overflow-hidden bg-[#0a0a09]">
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
            <div className="shadow-[0_40px_80px_-30px_rgba(0,0,0,0.8)]">
              <GameCover title={game.title} coverUrl={game.coverUrl} index={game.id} year={formatYear(game.releaseDate)} genre={game.genres[0]} />
            </div>
            {isInstalled && (
              <p className="mt-3 flex flex-wrap items-center gap-2 text-[11.5px] text-ink-3">
                <span className={cn("size-1.5 rounded-full", isAdopted ? "bg-signal" : "bg-ok")} aria-hidden="true" />
                {t("Installed")} {formatBytes(isInstalled.sizeBytes)}
                {isAdopted && <Badge tone="signal">{t("Adopted")}</Badge>}
              </p>
            )}
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
              {isNative() && !isInstalled && !activeJob && (
                <Button
                  variant="default"
                  size="lg"
                  icon={<Link2 className="size-4" />}
                  onClick={() => setAdoptOpen(true)}
                >
                  {t("Link existing installation")}
                </Button>
              )}
              {streamTarget.data?.published && (
                <Button variant="default" size="lg" icon={<Cast className="size-4" />} onClick={() => setStreamOpen(true)}>
                  {t("Stream")}
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
              {game.versions.length > 1 && version && (
                <div className="w-[190px]">
                  <Select
                    label={t("Game version")}
                    value={version.id}
                    onChange={setVersionId}
                    options={game.versions.map((v) => ({
                      value: v.id,
                      label: v.version ? `v${v.version}` : v.fileName,
                      hint: formatBytes(v.size),
                    }))}
                    className="h-12 text-[13px]"
                  />
                </div>
              )}
              <Button
                variant="default"
                icon={<ListPlus className="size-4" />}
                onClick={() => setCollectionOpen(true)}
                className="h-12"
              >
                {t("Add to collection")}
              </Button>
              {isEditor && (
                <>
                  <Button
                    variant="default"
                    icon={<Palette className="size-4" />}
                    onClick={() => setArtworkOpen(true)}
                    className="h-12"
                  >
                    Artwork
                  </Button>
                  <Button
                    variant="default"
                    icon={<Share2 className="size-4" />}
                    onClick={() => setShareOpen(true)}
                    className="h-12"
                  >
                    {t("Share")}
                  </Button>
                </>
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
              {menu.length > 0 && <DetailMenu items={menu} />}
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
              {isInstalled && (
                <button
                  type="button"
                  onClick={() => void native.openPath(isInstalled.installPath).catch((cause: Error) => push({ title: t("Could not open the folder"), description: cause.message, tone: "danger" }))}
                  className="text-[12px] text-ink-3 underline-offset-4 hover:text-ink hover:underline"
                >
                  {t("Open folder")}
                </button>
              )}
            </div>
          </div>
        </div>
        </div>
      </section>
      <div className="relative z-10 -mt-16 h-16 bg-gradient-to-b from-transparent to-bg" aria-hidden="true" />

      <div className="page pt-8">
        <MediaGallery game={game} spec={spec} seed={hashString(game.title)} />

        <AchievementsPanel gameId={game.id} isEditor={isEditor} installPath={isInstalled?.installPath} />

        <div className="mt-14 grid gap-12 lg:grid-cols-[1fr_320px]">
          <div>
            <section aria-labelledby="files-heading">
              <SectionHeader title={t("Files")} count={game.versions.length} description={t("Archives on the server")} />
              <div className="border-t border-line" id="files-heading">
                {game.versions.map((v) => (
                  <div key={v.id} className="flex flex-wrap items-baseline justify-between gap-3 border-b border-line py-3">
                    <div className="min-w-0">
                      <p className="truncate text-[13.5px] text-ink">{v.fileName}</p>
                      <p className="mt-0.5 flex flex-wrap items-baseline gap-x-3 font-mono text-[10.5px] text-ink-3">
                        <span>{v.version ? `v${v.version}` : "unversioned"}</span>
                        <span>{formatBytes(v.size)}</span>
                        <span>{formatDate(v.releaseDate)}</span>
                        {v.earlyAccess && <span className="text-signal-text">{t("early access")}</span>}
                      </p>
                    </div>
                    <Button
                      variant="ghost"
                      size="sm"
                      icon={<Download className="size-3.5" />}
                      onClick={() => void download()}
                      className="text-ink-2"
                    >
                      {t("Download")}
                    </Button>
                  </div>
                ))}
              </div>
            </section>

            {isInstalled && isNative() && <InstallFiles gameId={game.id} title={game.title} />}

            {isInstalled && isNative() && (
              <InstallerPanel gameId={game.id} title={game.title} executable={isInstalled.executable} />
            )}

            {isInstalled && isNative() && (
              <LaunchOptionsPanel gameId={game.id} title={game.title} />
            )}

            {isInstalled && isNative() && (
              <GameSettingsPanel gameId={game.id} title={game.title} />
            )}

            <CloudSaves gameId={game.id} title={game.title} />

            <RomPanel game={game} />

            <div className="mt-10">
              <ControllerPanel gameId={game.id} gameTitle={game.title} />
            </div>

            {isEditor && (
              <section className="mt-12 scroll-mt-6" id="launch-options" aria-labelledby="launch-heading">
                <SectionHeader
                  title={t("Launch options")}
                  description={t("Used by the desktop app when it starts the game")}
                  actions={
                    <Button variant="ghost" size="sm" onClick={() => setLaunchOpen((open) => !open)}>
                      {launchOpen ? t("Hide") : t("Edit")}
                    </Button>
                  }
                />
                <div id="launch-heading" className="border-t border-line pt-4">
                  {launchOpen ? (
                    <LaunchForm game={game} installed={isInstalled} onSaved={() => void queryClient.invalidateQueries({ queryKey: ["game", game.id] })} />
                  ) : (
                    <dl className="grid gap-x-8 gap-y-2 sm:grid-cols-2">
                      {[
                        [t("Executable"), game.launch.executable ?? t("Not set")],
                        [t("Parameters"), game.launch.parameters ?? t("None")],
                        [t("Installer"), game.launch.installerExecutable ?? t("None")],
                        [t("Uninstaller"), game.launch.uninstallerExecutable ?? t("None")],
                      ].map(([label, value]) => (
                        <div key={label} className="flex items-baseline justify-between gap-4 border-b border-line py-2">
                          <dt className="text-[12px] text-ink-3">{label}</dt>
                          <dd className="truncate font-mono text-[11px] text-ink-2">{value}</dd>
                        </div>
                      ))}
                    </dl>
                  )}
                </div>
              </section>
            )}
          </div>

          <aside className="flex flex-col gap-10">
            <section aria-labelledby="details-heading">
              <SectionHeader title={t("Details")} />
              <dl id="details-heading" className="border-t border-line">
                {[
                  [t("Developer"), game.developers.join(", ") || "—"],
                  [t("Publisher"), game.publishers.join(", ") || "—"],
                  [t("Released"), formatDate(game.releaseDate)],
                  [t("Genres"), game.genres.join(", ")],
                  [t("Platform"), game.platform ?? "—"],
                  [t("Age rating"), game.ageRating ? `${game.ageRating}+` : t("Not rated")],
                ].map(([label, value]) => (
                  <div key={label} className="flex items-baseline justify-between gap-4 border-b border-line py-2.5">
                    <dt className="shrink-0 text-[12px] text-ink-3">{label}</dt>
                    <dd className="text-right text-[12.5px] text-ink-2">{value}</dd>
                  </div>
                ))}
              </dl>
              {game.tags.length > 0 && (
                <div className="mt-3 flex flex-wrap gap-1.5">
                  {game.tags.map((tag) => (
                    <span key={tag} className="border border-line px-2 py-1 text-[11px] text-ink-3">
                      {tag}
                    </span>
                  ))}
                </div>
              )}
            </section>

            <section aria-labelledby="progress-heading">
              <SectionHeader title={t("Your progress")} />
              <div id="progress-heading" className="flex flex-col gap-3 border-t border-line pt-4">
                <div className="flex items-baseline justify-between gap-4">
                  <span className="text-[12px] text-ink-3">{t("State")}</span>
                  <span className={cn("flex items-center gap-1.5 text-[12.5px]", STATE_TONE[game.state].text)}>
                    <span className={cn("size-1.5 rounded-full", STATE_TONE[game.state].dot)} aria-hidden="true" />
                    {stateLabel(game.state)}
                  </span>
                </div>
                <div className="flex items-baseline justify-between gap-4">
                  <span className="text-[12px] text-ink-3">{t("Playtime")}</span>
                  <span className="num text-[12.5px] text-ink-2">{totalPlayed ?? t("Not played")}</span>
                </div>
                <div className="flex items-baseline justify-between gap-4">
                  <span className="text-[12px] text-ink-3">{t("Last session")}</span>
                  <span className="text-[12.5px] text-ink-2">{formatRelative(game.lastPlayedAt)}</span>
                </div>
                {isNative() && playtime.data && playtime.data.sessions > 0 && (
                  <div className="pt-2">
                    <div className="flex items-baseline justify-between gap-4">
                      <span className="text-[12px] text-ink-3">{t("Sessions on this device")}</span>
                      <span className="num text-[12.5px] text-ink-2">{playtime.data.sessions}</span>
                    </div>
                    <ul className="mt-2 border-t border-line">
                      {playtime.data.recentSessions.slice(0, 5).map((session) => (
                        <li
                          key={session.id}
                          className="flex items-baseline justify-between gap-3 border-b border-line py-1.5 font-mono text-[10.5px] text-ink-3"
                        >
                          <span>{formatDate(session.startedAt)}</span>
                          <span className="text-ink-2">{formatSeconds(session.seconds) ?? `${session.minutes}m`}</span>
                        </li>
                      ))}
                    </ul>
                  </div>
                )}
                {user && <p className="pt-1 text-[11.5px] leading-relaxed text-ink-3">{t("Progress is stored per account on the server.")}</p>}
              </div>
            </section>
          </aside>
        </div>
      </div>

      {version && (
        <InstallDialog open={installOpen} onOpenChange={setInstallOpen} game={game} version={version} />
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

function LaunchForm({ game, installed, onSaved }: { game: GameDetail; installed?: InstalledGame; onSaved: () => void }) {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const [executable, setExecutable] = useState(game.launch.executable ?? "");
  const [parameters, setParameters] = useState(game.launch.parameters ?? "");
  const [installer, setInstaller] = useState(game.launch.installerExecutable ?? "");
  const [uninstaller, setUninstaller] = useState(game.launch.uninstallerExecutable ?? "");
  const [busy, setBusy] = useState(false);

  // The picker needs a folder on this device; a game installed elsewhere (or
  // not installed yet) keeps the manual fields (#143).
  const canPick = Boolean(installed && isNative());

  const save = async () => {
    setBusy(true);
    try {
      let nextExecutable = executable.trim();
      let nextInstaller = installer.trim();
      let nextUninstaller = uninstaller.trim();
      if (canPick) {
        // Validate against the install folder before writing the server
        // config: a path that escapes it is refused here, not at launch.
        // An imported row without a folder on this device has nothing to
        // validate against, so its manual values are kept as typed.
        const listing = await native.listGameExecutables(game.id).catch((cause: unknown) => {
          const message = errorMessage(cause);
          if (message.includes("install folder is missing") || message.includes("has no folder")) return null;
          throw cause;
        });
        if (listing) {
          if (nextExecutable) nextExecutable = await native.checkGameExecutable(game.id, nextExecutable);
          if (nextInstaller) nextInstaller = await native.checkGameExecutable(game.id, nextInstaller);
          if (nextUninstaller) nextUninstaller = await native.checkGameExecutable(game.id, nextUninstaller);
          setExecutable(nextExecutable);
          setInstaller(nextInstaller);
          setUninstaller(nextUninstaller);
        }
      }
      await api.patchGame(game.id, {
        launch: {
          executable: nextExecutable || null,
          parameters: parameters || null,
          installerExecutable: nextInstaller || null,
          uninstallerExecutable: nextUninstaller || null,
        },
      });
      // The desktop app launches `installed.executable` when one is recorded,
      // so the picker must update that row too; otherwise the saved choice
      // would never be used (#143).
      let localExecutableSaved = true;
      if (canPick && nextExecutable) {
        try {
          await native.setExecutable(game.id, nextExecutable);
          await queryClient.invalidateQueries({ queryKey: ["installed"] });
        } catch (cause) {
          localExecutableSaved = false;
          push({
            title: t("Launch options saved"),
            description: errorMessage(cause),
            tone: "default",
          });
        }
      }
      if (localExecutableSaved) push({ title: t("Launch options saved"), tone: "success" });
      onSaved();
    } catch (cause) {
      push({ title: t("Could not save"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="grid gap-4 sm:grid-cols-2">
      {canPick ? (
        <div className="sm:col-span-2">
          <ExecutableField
            gameId={game.id}
            kind="launch"
            label={t("Executable")}
            value={executable}
            onChange={setExecutable}
            autoSelectRecommended
            hint={t("Programs found in the install folder. The detected one is marked.")}
          />
        </div>
      ) : (
        <label className="flex flex-col gap-1.5">
          <span className="text-[12px] text-ink-2">{t("Executable")}</span>
          <Input value={executable} onChange={(event) => setExecutable(event.target.value)} placeholder="Game.exe" className="h-10 font-mono text-[12px]" />
        </label>
      )}
      <label className={cn("flex flex-col gap-1.5", canPick && "sm:col-span-2")}>
        <span className="text-[12px] text-ink-2">{t("Parameters")}</span>
        <Input value={parameters} onChange={(event) => setParameters(event.target.value)} placeholder="-windowed" className="h-10 font-mono text-[12px]" />
      </label>
      {canPick ? (
        <>
          <div className="sm:col-span-2">
            <ExecutableField
              gameId={game.id}
              kind="installer"
              label={t("Installer")}
              value={installer}
              onChange={setInstaller}
              hint={t("setup, install and gog programs found in the install folder.")}
            />
          </div>
          <div className="sm:col-span-2">
            <ExecutableField
              gameId={game.id}
              kind="uninstaller"
              label={t("Uninstaller")}
              value={uninstaller}
              onChange={setUninstaller}
              hint={t("unins and uninstall programs found in the install folder.")}
            />
          </div>
        </>
      ) : (
        <>
          <label className="flex flex-col gap-1.5">
            <span className="text-[12px] text-ink-2">{t("Installer")}</span>
            <Input value={installer} onChange={(event) => setInstaller(event.target.value)} placeholder="setup.exe" className="h-10 font-mono text-[12px]" />
          </label>
          <label className="flex flex-col gap-1.5">
            <span className="text-[12px] text-ink-2">{t("Uninstaller")}</span>
            <Input value={uninstaller} onChange={(event) => setUninstaller(event.target.value)} placeholder="unins000.exe" className="h-10 font-mono text-[12px]" />
          </label>
        </>
      )}
      <div className="sm:col-span-2">
        <Button variant="default" loading={busy} onClick={() => void save()}>
          {t("Save launch options")}
        </Button>
      </div>
    </div>
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
