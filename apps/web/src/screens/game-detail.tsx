import { useMemo, useState } from "react";
import { useLocation, useNavigate, useParams } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  ArrowLeft,
  Bookmark,
  BookmarkCheck,
  ChevronLeft,
  ChevronRight,
  Clock,
  Download,
  FolderInput,
  FolderOpen,
  MoreHorizontal,
  Pencil,
  Play,
  RefreshCw,
  Trash2,
} from "lucide-react";
import { api, assetUrl, USING_MOCKS } from "@/lib/api";
import { ApiError, errorMessage } from "@/lib/http";
import { cn } from "@/lib/cn";
import { formatBytes, formatDate, formatEta, formatMinutes, formatRating, formatRelative, formatSpeed, formatYear, stateLabel } from "@/lib/format";
import { isNative, native } from "@/lib/native";
import type { GameDetail, ProgressState } from "@/lib/types";
import { useBookmark, useInstalledMap, useLaunchGame } from "@/lib/hooks";
import { useDownloads } from "@/state/downloads";
import { CloudSaves } from "@/components/game/cloud-saves";
import { InstallDialog } from "@/components/game/install-dialog";
import { InstallFiles } from "@/components/game/install-files";
import { MoveInstallDialog } from "@/components/game/move-install";
import { useAuth, useIsEditor } from "@/state/auth";
import { useToast } from "@/state/toasts";
import { GameBackdrop, GameCover, StillPlate } from "@/components/art/cover";
import { artFromUrl, hashString } from "@/lib/art";
import { STATE_TONE } from "@/components/game/state";
import { ErrorState, SectionHeader } from "@/components/page";
import { Badge, Button, IconButton, Input, Progress, Skeleton } from "@/components/ui/controls";
import { Dialog } from "@/components/ui/dialog";
import { DropdownMenu, Select, useDropdown } from "@/components/ui/select";

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
  const [installOpen, setInstallOpen] = useState(false);
  const [moveOpen, setMoveOpen] = useState(false);
  const [lightbox, setLightbox] = useState<number | null>(null);
  const [launchOpen, setLaunchOpen] = useState(false);

  const { data: game, isPending, isError, error, refetch } = useQuery({
    queryKey: ["game", gameId],
    queryFn: () => api.getGame(gameId),
    enabled: Number.isFinite(gameId),
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
          title={notFound ? "Not in the library" : "This game didn’t load"}
          message={notFound ? "That game is no longer indexed. It may have been removed from the games folder." : errorMessage(error)}
          onRetry={() => void refetch()}
          action={
            <Button variant="ghost" onClick={back}>
              Back
            </Button>
          }
        />
      </div>
    );
  }

  if (!game) return null;

  const isInstalled = installed.get(game.id);
  const activeJob = downloads.jobForGame(game.id);
  const resumableJob = downloads.resumableForGame(game.id);
  const installPct = activeJob?.bytesTotal ? Math.min(100, (activeJob.bytesDone / activeJob.bytesTotal) * 100) : null;
  const canPlay = isNative() && Boolean(isInstalled);
  const spec = artFromUrl(game.backgroundUrl, game.title);

  const setProgress = async (state: ProgressState) => {
    try {
      await api.setProgress(game.id, state);
      void queryClient.invalidateQueries({ queryKey: ["game", game.id] });
      void queryClient.invalidateQueries({ queryKey: ["games"] });
      void queryClient.invalidateQueries({ queryKey: ["stats"] });
    } catch (cause) {
      push({ title: "Progress not saved", description: errorMessage(cause), tone: "danger" });
    }
  };

  const download = async () => {
    if (USING_MOCKS) {
      push({ title: "Demo build", description: "Downloads and installs need the real server. The browser flow is wired." });
      return;
    }
    try {
      const ticket = await api.downloadTicket(game.id, version?.id);
      const url = assetUrl(ticket.url);
      if (!url) throw new Error("The server returned an empty download URL");
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.rel = "noopener";
      anchor.click();
    } catch (cause) {
      push({ title: "Download did not start", description: errorMessage(cause), tone: "danger" });
    }
  };

  const primary = canPlay ? (
    <Button
      variant="primary"
      size="lg"
      icon={<Play className="size-4" />}
      onClick={() => launch.mutate({ id: game.id, title: game.title, executable: isInstalled?.executable ?? game.launch.executable })}
    >
      Play
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
      Resume install
    </Button>
  ) : activeJob ? (
    <Button variant="default" size="lg" icon={<Download className="size-4" />} onClick={() => navigate("/downloads")}>
      Installing {installPct !== null ? `${installPct.toFixed(0)}%` : ""}
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
      Resume install
    </Button>
  ) : isNative() ? (
    <Button variant="primary" size="lg" icon={<Download className="size-4" />} onClick={() => setInstallOpen(true)}>
      Install
    </Button>
  ) : (
    <Button variant="primary" size="lg" icon={<Download className="size-4" />} onClick={() => void download()}>
      Download
    </Button>
  );

  const menu = [
    ...(isEditor
      ? [
          {
            label: "Refresh metadata",
            icon: <RefreshCw className="size-3.5" />,
            onSelect: () => {
              void api
                .refreshMetadata(game.id)
                .then(() => push({ title: "Metadata queued", description: "The server will fetch a fresh copy in the background.", tone: "success" }))
                .catch((cause: Error) => push({ title: "Could not queue metadata", description: cause.message, tone: "danger" }));
            },
          },
          {
            label: "Edit launch options",
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
            label: "Open install folder",
            icon: <FolderOpen className="size-3.5" />,
            onSelect: () => void native.openPath(isInstalled.installPath).catch((cause: Error) => push({ title: "Could not open the folder", description: cause.message, tone: "danger" })),
          },
          {
            label: "Move install…",
            icon: <FolderInput className="size-3.5" />,
            onSelect: () => setMoveOpen(true),
          },
          {
            label: "Uninstall",
            icon: <Trash2 className="size-3.5" />,
            danger: true,
            onSelect: () => {
              void native
                .uninstallGame(game.id)
                .then(() => {
                  void queryClient.invalidateQueries({ queryKey: ["installed"] });
                  push({ title: "Game removed", description: `${game.title} was uninstalled from this machine.` });
                })
                .catch((cause: Error) => push({ title: "Could not uninstall", description: cause.message, tone: "danger" }));
            },
          },
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
          <ArrowLeft className="size-3.5" aria-hidden="true" />
          Back
        </button>

        <div className="mt-[16vh] grid gap-8 lg:mt-[13vh] lg:grid-cols-[250px_1fr] lg:gap-12 xl:grid-cols-[280px_1fr]">
          <div className="w-[150px] sm:w-[190px] lg:w-full">
            <div className="shadow-[0_40px_80px_-30px_rgba(0,0,0,0.8)]">
              <GameCover title={game.title} coverUrl={game.coverUrl} index={game.id} year={formatYear(game.releaseDate)} genre={game.genres[0]} />
            </div>
            {isInstalled && (
              <p className="mt-3 flex items-center gap-2 text-[11.5px] text-ink-3">
                <span className="size-1.5 rounded-full bg-ok" aria-hidden="true" />
                Installed {formatBytes(isInstalled.sizeBytes)}
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
              {game.earlyAccess && <Badge tone="signal">Early access</Badge>}
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
              <span>{game.versionCount === 1 ? "1 version" : `${game.versionCount} versions`}</span>
              {formatMinutes(game.minutesPlayed) && (
                <span className="flex items-center gap-1.5">
                  <Clock className="size-3.5 text-ink-3" aria-hidden="true" />
                  <span className="num">{formatMinutes(game.minutesPlayed)}</span>
                </span>
              )}
              {game.lastPlayedAt && <span className="text-ink-3">Last played {formatRelative(game.lastPlayedAt)}</span>}
            </div>

            {game.description && <p className="measure mt-6 text-[14.5px] leading-relaxed text-ink-2">{game.description}</p>}

            <div className="mt-7 flex flex-wrap items-center gap-2">
              {primary}
              {game.versions.length > 1 && version && (
                <div className="w-[190px]">
                  <Select
                    label="Game version"
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
              <IconButton
                label={game.bookmarked ? "Remove bookmark" : "Bookmark"}
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
                <Progress value={installPct} label="Install progress" />
                <p className="mt-1.5 flex flex-wrap items-baseline gap-x-2.5 font-mono text-[10.5px] text-ink-3">
                  <span className="text-ink-2">
                    {activeJob.phase === "downloading" ? "Downloading" : activeJob.phase === "paused" ? "Paused" : activeJob.phase}
                  </span>
                  {installPct !== null && <span>{installPct.toFixed(0)}%</span>}
                  {activeJob.speedBps > 0 && <span>{formatSpeed(activeJob.speedBps)}</span>}
                  {activeJob.bytesTotal && activeJob.speedBps > 0 && (
                    <span>{formatEta(activeJob.bytesDone, activeJob.bytesTotal, activeJob.speedBps)} left</span>
                  )}
                </p>
              </div>
            )}

            <div className="mt-5 flex flex-wrap items-center gap-3">
              <StatePicker state={game.state} onChange={setProgress} />
              {isInstalled && (
                <button
                  type="button"
                  onClick={() => void native.openPath(isInstalled.installPath).catch((cause: Error) => push({ title: "Could not open the folder", description: cause.message, tone: "danger" }))}
                  className="text-[12px] text-ink-3 underline-offset-4 hover:text-ink hover:underline"
                >
                  Open folder
                </button>
              )}
            </div>
          </div>
        </div>
        </div>
      </section>
      <div className="relative z-10 -mt-16 h-16 bg-gradient-to-b from-transparent to-bg" aria-hidden="true" />

      <div className="page pt-8">
        {game.screenshots.length > 0 && (
          <section aria-labelledby="media-heading">
            <SectionHeader title="Media" count={game.screenshots.length} />
            <div id="media-heading" className="nav-scroll -mx-[var(--page-x)] flex gap-3 overflow-x-auto px-[var(--page-x)] pb-2">
              {game.screenshots.map((url, index) => (
                <button
                  key={url}
                  type="button"
                  onClick={() => setLightbox(index)}
                  className="group relative w-[240px] shrink-0 overflow-hidden rounded-[2px] border border-line text-left transition-colors hover:border-line-2 sm:w-[300px]"
                  aria-label={`Open still ${index + 1}`}
                >
                  <ScreenshotPlate url={url} spec={spec} seed={hashString(game.title)} index={index} />
                </button>
              ))}
            </div>
          </section>
        )}

        <div className="mt-14 grid gap-12 lg:grid-cols-[1fr_320px]">
          <div>
            <section aria-labelledby="files-heading">
              <SectionHeader title="Files" count={game.versions.length} description="Archives on the server" />
              <div className="border-t border-line" id="files-heading">
                {game.versions.map((v) => (
                  <div key={v.id} className="flex flex-wrap items-baseline justify-between gap-3 border-b border-line py-3">
                    <div className="min-w-0">
                      <p className="truncate text-[13.5px] text-ink">{v.fileName}</p>
                      <p className="mt-0.5 flex flex-wrap items-baseline gap-x-3 font-mono text-[10.5px] text-ink-3">
                        <span>{v.version ? `v${v.version}` : "unversioned"}</span>
                        <span>{formatBytes(v.size)}</span>
                        <span>{formatDate(v.releaseDate)}</span>
                        {v.earlyAccess && <span className="text-signal-text">early access</span>}
                      </p>
                    </div>
                    <Button
                      variant="ghost"
                      size="sm"
                      icon={<Download className="size-3.5" />}
                      onClick={() => void download()}
                      className="text-ink-2"
                    >
                      Download
                    </Button>
                  </div>
                ))}
              </div>
            </section>

            {isInstalled && isNative() && <InstallFiles gameId={game.id} title={game.title} />}

            <CloudSaves gameId={game.id} title={game.title} />

            {isEditor && (
              <section className="mt-12 scroll-mt-6" id="launch-options" aria-labelledby="launch-heading">
                <SectionHeader
                  title="Launch options"
                  description="Used by the desktop app when it starts the game"
                  actions={
                    <Button variant="ghost" size="sm" onClick={() => setLaunchOpen((open) => !open)}>
                      {launchOpen ? "Hide" : "Edit"}
                    </Button>
                  }
                />
                <div id="launch-heading" className="border-t border-line pt-4">
                  {launchOpen ? (
                    <LaunchForm game={game} onSaved={() => void queryClient.invalidateQueries({ queryKey: ["game", game.id] })} />
                  ) : (
                    <dl className="grid gap-x-8 gap-y-2 sm:grid-cols-2">
                      {[
                        ["Executable", game.launch.executable ?? "Not set"],
                        ["Parameters", game.launch.parameters ?? "None"],
                        ["Installer", game.launch.installerExecutable ?? "None"],
                        ["Uninstaller", game.launch.uninstallerExecutable ?? "None"],
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
              <SectionHeader title="Details" />
              <dl id="details-heading" className="border-t border-line">
                {[
                  ["Developer", game.developers.join(", ") || "—"],
                  ["Publisher", game.publishers.join(", ") || "—"],
                  ["Released", formatDate(game.releaseDate)],
                  ["Genres", game.genres.join(", ")],
                  ["Age rating", game.ageRating ? `${game.ageRating}+` : "Not rated"],
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
              <SectionHeader title="Your progress" />
              <div id="progress-heading" className="flex flex-col gap-3 border-t border-line pt-4">
                <div className="flex items-baseline justify-between gap-4">
                  <span className="text-[12px] text-ink-3">State</span>
                  <span className={cn("flex items-center gap-1.5 text-[12.5px]", STATE_TONE[game.state].text)}>
                    <span className={cn("size-1.5 rounded-full", STATE_TONE[game.state].dot)} aria-hidden="true" />
                    {stateLabel(game.state)}
                  </span>
                </div>
                <div className="flex items-baseline justify-between gap-4">
                  <span className="text-[12px] text-ink-3">Playtime</span>
                  <span className="num text-[12.5px] text-ink-2">{formatMinutes(game.minutesPlayed) ?? "Not played"}</span>
                </div>
                <div className="flex items-baseline justify-between gap-4">
                  <span className="text-[12px] text-ink-3">Last session</span>
                  <span className="text-[12.5px] text-ink-2">{formatRelative(game.lastPlayedAt)}</span>
                </div>
                {user && <p className="pt-1 text-[11.5px] leading-relaxed text-ink-3">Progress is stored per account on the server.</p>}
              </div>
            </section>
          </aside>
        </div>
      </div>

      {version && (
        <InstallDialog open={installOpen} onOpenChange={setInstallOpen} game={game} version={version} />
      )}

      {isInstalled && (
        <MoveInstallDialog
          open={moveOpen}
          onOpenChange={setMoveOpen}
          gameId={game.id}
          title={game.title}
          installed={isInstalled}
          onMoved={() => {
            void queryClient.invalidateQueries({ queryKey: ["installed"] });
            void queryClient.invalidateQueries({ queryKey: ["game", game.id] });
          }}
        />
      )}

      <Dialog
        open={lightbox !== null}
        onOpenChange={(open) => !open && setLightbox(null)}
        title={`${game.title} — still ${(lightbox ?? 0) + 1}`}
        size="lg"
      >
        {lightbox !== null && (
          <div className="flex flex-col gap-3">
            <ScreenshotPlate url={game.screenshots[lightbox]} spec={spec} seed={hashString(game.title)} index={lightbox} />
            <div className="flex items-center justify-between">
              <span className="font-mono text-[11px] text-ink-3">
                {lightbox + 1} / {game.screenshots.length}
              </span>
              <div className="flex gap-2">
                <Button
                  variant="default"
                  size="sm"
                  icon={<ChevronLeft className="size-3.5" />}
                  disabled={lightbox === 0}
                  onClick={() => setLightbox((current) => Math.max(0, (current ?? 0) - 1))}
                >
                  Previous
                </Button>
                <Button
                  variant="default"
                  size="sm"
                  iconRight={<ChevronRight className="size-3.5" />}
                  disabled={lightbox === game.screenshots.length - 1}
                  onClick={() => setLightbox((current) => Math.min(game.screenshots.length - 1, (current ?? 0) + 1))}
                >
                  Next
                </Button>
              </div>
            </div>
          </div>
        )}
      </Dialog>
    </div>
  );
}

/* --------------------------------- pieces -------------------------------- */

function ScreenshotPlate({
  url,
  spec,
  seed,
  index,
}: {
  url: string;
  spec: ReturnType<typeof artFromUrl>;
  seed: number;
  index: number;
}) {
  const src = assetUrl(url);
  const demo = !url || url.startsWith("demo://");
  return (
    <div className="media-plate aspect-video w-full">
      {demo ? (
        <StillPlate spec={spec} seed={seed} index={index} className="absolute inset-0 h-full w-full" />
      ) : (
        <img src={src ?? ""} alt="" className="h-full w-full object-cover" loading="lazy" />
      )}
    </div>
  );
}

function DetailMenu({ items }: { items: { label: string; icon?: React.ReactNode; danger?: boolean; onSelect: () => void }[] }) {
  const menu = useDropdown();
  return (
    <>
      <IconButton {...menu.triggerProps} label="More actions" variant="default" className="size-12">
        <MoreHorizontal className="size-4" />
      </IconButton>
      <DropdownMenu {...menu.menuProps} items={items} label="Game actions" />
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
        <span className="text-ink-3">Progress</span>
        <span className="text-ink">{stateLabel(state)}</span>
      </button>
      <DropdownMenu
        {...menu.menuProps}
        label="Set progress state"
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

function LaunchForm({ game, onSaved }: { game: GameDetail; onSaved: () => void }) {
  const { push } = useToast();
  const [executable, setExecutable] = useState(game.launch.executable ?? "");
  const [parameters, setParameters] = useState(game.launch.parameters ?? "");
  const [installer, setInstaller] = useState(game.launch.installerExecutable ?? "");
  const [uninstaller, setUninstaller] = useState(game.launch.uninstallerExecutable ?? "");
  const [busy, setBusy] = useState(false);

  const save = async () => {
    setBusy(true);
    try {
      await api.patchGame(game.id, {
        launch: {
          executable: executable || null,
          parameters: parameters || null,
          installerExecutable: installer || null,
          uninstallerExecutable: uninstaller || null,
        },
      });
      push({ title: "Launch options saved", tone: "success" });
      onSaved();
    } catch (cause) {
      push({ title: "Could not save", description: errorMessage(cause), tone: "danger" });
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="grid gap-4 sm:grid-cols-2">
      <label className="flex flex-col gap-1.5">
        <span className="text-[12px] text-ink-2">Executable</span>
        <Input value={executable} onChange={(event) => setExecutable(event.target.value)} placeholder="Game.exe" className="h-10 font-mono text-[12px]" />
      </label>
      <label className="flex flex-col gap-1.5">
        <span className="text-[12px] text-ink-2">Parameters</span>
        <Input value={parameters} onChange={(event) => setParameters(event.target.value)} placeholder="-windowed" className="h-10 font-mono text-[12px]" />
      </label>
      <label className="flex flex-col gap-1.5">
        <span className="text-[12px] text-ink-2">Installer</span>
        <Input value={installer} onChange={(event) => setInstaller(event.target.value)} placeholder="setup.exe" className="h-10 font-mono text-[12px]" />
      </label>
      <label className="flex flex-col gap-1.5">
        <span className="text-[12px] text-ink-2">Uninstaller</span>
        <Input value={uninstaller} onChange={(event) => setUninstaller(event.target.value)} placeholder="unins000.exe" className="h-10 font-mono text-[12px]" />
      </label>
      <div className="sm:col-span-2">
        <Button variant="default" loading={busy} onClick={() => void save()}>
          Save launch options
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
