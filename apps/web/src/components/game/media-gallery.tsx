import { useCallback, useEffect, useRef, useState } from "react";
import { ChevronLeft, ChevronRight, ExternalLink, Maximize2, Minimize2, Play } from "lucide-react";
import { useReducedMotion } from "motion/react";
import { assetUrl } from "@/lib/api";
import { cn } from "@/lib/cn";
import type { ArtSpec } from "@/lib/art";
import type { GameDetail, GameVideo } from "@/lib/types";
import { StillPlate } from "@/components/art/cover";
import { Button, IconButton } from "@/components/ui/controls";
import { Dialog } from "@/components/ui/dialog";
import { SectionHeader } from "@/components/page";
import { t } from "@/lib/i18n";

type MediaItem =
  | { kind: "video"; video: GameVideo }
  | { kind: "still"; url: string; index: number };

function youtubeEmbed(videoId: string): string {
  return `https://www.youtube-nocookie.com/embed/${videoId}?rel=0&modestbranding=1&autoplay=1`;
}

/** Rows enriched before the provider field existed still carry YouTube URLs. */
function youtubeId(video: GameVideo): string | null {
  if (video.videoId) return video.videoId;
  const match = /(?:youtube\.com\/watch\?v=|youtu\.be\/)([A-Za-z0-9_-]{6,})/.exec(video.url);
  return match?.[1] ?? null;
}

function videoPoster(video: GameVideo): string | null {
  if (video.thumbnailUrl) return video.thumbnailUrl;
  const id = youtubeId(video);
  if (id) return `https://img.youtube.com/vi/${id}/hqdefault.jpg`;
  return null;
}

/** Public watch page, used when the owner disabled embedding or blocks the region. */
function youtubeWatchUrl(video: GameVideo): string | null {
  const id = youtubeId(video);
  if (id) return `https://www.youtube.com/watch?v=${id}`;
  if (/^https?:\/\/(?:www\.)?(?:youtube\.com|youtu\.be)\//.test(video.url)) return video.url;
  return null;
}

/**
 * The iframe cannot tell "embedding disabled", "region blocked" and "CSP
 * failure" apart, so the external link stays visible next to the player (#152).
 * It never replaces the player and never starts playback by itself.
 */
function TrailerFallback({ video }: { video: GameVideo }) {
  const watch = youtubeWatchUrl(video);
  if (!watch) return null;
  return (
    <p className="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1 text-[11.5px] text-ink-3">
      <span>{t("If the trailer does not play, its owner may have disabled embedding or blocked it in your region.")}</span>
      <a
        href={watch}
        target="_blank"
        rel="noreferrer"
        className="inline-flex items-center gap-1 text-signal-text underline-offset-2 hover:underline"
      >
        {t("Open on YouTube")}
        <ExternalLink className="size-3" aria-hidden="true" />
      </a>
    </p>
  );
}

/** Click-to-play trailer. Nothing loads from YouTube until the viewer presses
 * play, and playback starts only from that gesture. */
export function TrailerPlayer({
  video,
  title,
  className,
}: {
  video: GameVideo;
  title: string;
  className?: string;
}) {
  const [playing, setPlaying] = useState(false);
  const reduce = useReducedMotion();
  const poster = videoPoster(video);
  const id = youtubeId(video);
  const isYoutube = Boolean(id);

  useEffect(() => {
    setPlaying(false);
  }, [video.videoId, video.url]);

  if (playing) {
    return (
      <div className={className}>
        <div className="media-plate aspect-video w-full overflow-hidden bg-stage">
          {isYoutube ? (
            <iframe
              src={youtubeEmbed(id!)}
              title={video.title || title}
              className="h-full w-full"
              allow="accelerometer; autoplay; clipboard-write; encrypted-media; gyroscope; picture-in-picture; web-share"
              referrerPolicy="strict-origin-when-cross-origin"
              allowFullScreen
              loading="lazy"
            />
          ) : (
            <video
              src={assetUrl(video.url) ?? undefined}
              controls
              autoPlay
              playsInline
              className="h-full w-full bg-stage"
            />
          )}
        </div>
        <TrailerFallback video={video} />
      </div>
    );
  }

  return (
    <div className={className}>
      <button
        type="button"
        onClick={() => setPlaying(true)}
        className="media-plate group relative block aspect-video w-full overflow-hidden bg-bg-deep text-left"
        aria-label={t("Play trailer: {title}", { title: video.title || title })}
      >
        {poster ? (
          <img
            src={poster}
            alt=""
            loading="lazy"
            className={cn(
              "h-full w-full object-cover opacity-90 transition-[opacity,transform] duration-[620ms]",
              !reduce && "group-hover:scale-[1.015] group-hover:opacity-100",
            )}
          />
        ) : (
          <StillPlate spec={{ palette: "graphite", motif: "horizon" }} seed={0} index={0} className="absolute inset-0 h-full w-full" />
        )}
        <span className="absolute inset-0 still-shade" aria-hidden="true" />
        <span className="absolute inset-0 grid place-items-center">
          <span
            className={cn(
              "media-control grid size-16 place-items-center rounded-full backdrop-blur-sm transition-transform duration-[180ms]",
              !reduce && "group-hover:scale-105",
            )}
          >
            <Play className="ml-0.5 size-6" aria-hidden="true" />
          </span>
        </span>
        <span className="absolute inset-x-0 bottom-0 flex items-end justify-between gap-4 p-4">
          <span className="min-w-0">
            <span className="block truncate text-[14px] font-medium text-sheen">{video.title || title}</span>
            <span className="mt-0.5 block font-mono text-[10px] uppercase tracking-[0.14em] text-sheen/65">
              {isYoutube ? t("YouTube · no cookies until play") : t("Video")}
            </span>
          </span>
          <span className="hidden shrink-0 border border-sheen/25 px-2 py-1 font-mono text-[10px] uppercase tracking-[0.12em] text-sheen/70 sm:block">
            {t("Play")}
          </span>
        </span>
      </button>
      <TrailerFallback video={video} />
    </div>
  );
}

function MediaFrame({
  item,
  game,
  spec,
  seed,
}: {
  item: MediaItem;
  game: GameDetail;
  spec: ArtSpec;
  seed: number;
}) {
  if (item.kind === "video") {
    return <TrailerPlayer video={item.video} title={game.title} />;
  }
  return (
    <div className="media-plate aspect-video w-full">
      {item.url.startsWith("demo://") ? (
        <StillPlate spec={spec} seed={seed} index={item.index} className="absolute inset-0 h-full w-full" />
      ) : (
        <img src={assetUrl(item.url) ?? ""} alt="" className="h-full w-full object-cover" loading="lazy" />
      )}
    </div>
  );
}

/** Static poster used by the strip: the play control belongs to the image,
 * not a nested button, so the card itself can open the lightbox. */
function MediaThumb({ item, game, spec, seed }: { item: MediaItem; game: GameDetail; spec: ArtSpec; seed: number }) {
  if (item.kind === "still") return <MediaFrame item={item} game={game} spec={spec} seed={seed} />;
  const poster = videoPoster(item.video);
  return (
    <div className="media-plate aspect-video w-full">
      {poster ? (
        <img src={poster} alt="" className="h-full w-full object-cover" loading="lazy" />
      ) : (
        <StillPlate spec={spec} seed={seed} index={0} className="absolute inset-0 h-full w-full" />
      )}
    </div>
  );
}

function MediaLightbox({
  open,
  onOpenChange,
  items,
  index,
  onIndexChange,
  game,
  spec,
  seed,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  items: MediaItem[];
  index: number;
  onIndexChange: (index: number) => void;
  game: GameDetail;
  spec: ArtSpec;
  seed: number;
}) {
  const stageRef = useRef<HTMLDivElement>(null);
  const [fullscreen, setFullscreen] = useState(false);
  const current = items[index];

  useEffect(() => {
    if (!open) {
      setFullscreen(false);
      return;
    }
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "ArrowLeft") {
        event.preventDefault();
        onIndexChange((index - 1 + items.length) % items.length);
      } else if (event.key === "ArrowRight") {
        event.preventDefault();
        onIndexChange((index + 1) % items.length);
      } else if (event.key === "Home") {
        event.preventDefault();
        onIndexChange(0);
      } else if (event.key === "End") {
        event.preventDefault();
        onIndexChange(items.length - 1);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [open, index, items.length, onIndexChange]);

  const toggleFullscreen = useCallback(async () => {
    const stage = stageRef.current;
    if (!stage) return;
    try {
      if (document.fullscreenElement) {
        await document.exitFullscreen();
        setFullscreen(false);
      } else {
        await stage.requestFullscreen();
        setFullscreen(true);
      }
    } catch {
      /* The browser can refuse fullscreen; the lightbox still works. */
    }
  }, []);

  if (!current) return null;

  return (
    <Dialog open={open} onOpenChange={onOpenChange} title={t("{title} — media", { title: game.title })} size="lg">
      <div className="flex flex-col gap-3">
        <div ref={stageRef} className="bg-bg-deep">
          <MediaFrame item={current} game={game} spec={spec} seed={seed} />
        </div>
        <div className="flex flex-wrap items-center justify-between gap-3">
          <span className="font-mono text-[11px] text-ink-3">
            {index + 1} / {items.length}
            {current.kind === "video" ? t(" · video") : t(" · still")}
          </span>
          <div className="flex flex-wrap gap-2">
            <Button
              variant="default"
              size="sm"
              icon={<ChevronLeft className="rtl:-scale-x-100 size-3.5" />}
              onClick={() => onIndexChange((index - 1 + items.length) % items.length)}
            >
              {t("Previous")}
            </Button>
            <Button
              variant="default"
              size="sm"
              iconRight={<ChevronRight className="rtl:-scale-x-100 size-3.5" />}
              onClick={() => onIndexChange((index + 1) % items.length)}
            >
              {t("Next")}
            </Button>
            <IconButton
              label={fullscreen ? t("Leave fullscreen") : t("Fullscreen")}
              variant="default"
              onClick={() => void toggleFullscreen()}
            >
              {fullscreen ? <Minimize2 className="size-4" /> : <Maximize2 className="size-4" />}
            </IconButton>
          </div>
        </div>
        <p className="text-[11.5px] text-ink-3">
          {t("Arrow keys move through the gallery, Escape closes it. Trailers only load after you press play.")}
        </p>
      </div>
    </Dialog>
  );
}

/** Trailer section and the combined screenshot + video gallery. */
export function MediaGallery({ game, spec, seed }: { game: GameDetail; spec: ArtSpec; seed: number }) {
  const videos = game.videos?.length ? game.videos : (game.trailers ?? []);
  const [selectedVideo, setSelectedVideo] = useState(0);
  const [lightbox, setLightbox] = useState<number | null>(null);
  const items: MediaItem[] = [
    ...videos.map((video) => ({ kind: "video" as const, video })),
    ...game.screenshots.map((url, index) => ({ kind: "still" as const, url, index })),
  ];
  const activeVideo = videos[selectedVideo] ?? videos[0];

  if (!items.length) return null;

  return (
    <>
      {activeVideo && (
        <section aria-labelledby="trailer-heading" className="mb-12">
          <SectionHeader
            title={t("Trailer")}
            count={videos.length}
            description={videos.length > 1 ? t("Choose a video, then press play") : undefined}
          />
          <div id="trailer-heading" className="grid gap-3 lg:grid-cols-[1fr_220px]">
            <TrailerPlayer video={activeVideo} title={game.title} />
            {videos.length > 1 && (
              <div className="flex gap-2 overflow-x-auto lg:flex-col lg:overflow-visible">
                {videos.map((video, index) => (
                  <button
                    key={`${video.url}-${index}`}
                    type="button"
                    onClick={() => setSelectedVideo(index)}
                    aria-pressed={index === selectedVideo}
                    className={cn(
                      "flex min-w-[170px] items-center gap-2 border p-2 text-left transition-colors lg:min-w-0",
                      index === selectedVideo ? "border-signal/60 bg-surface-2" : "border-line hover:border-line-2",
                    )}
                  >
                    <span className="relative aspect-video w-20 shrink-0 overflow-hidden bg-bg-deep">
                      {videoPoster(video) ? (
                        <img src={videoPoster(video)!} alt="" className="h-full w-full object-cover" loading="lazy" />
                      ) : (
                        <span className="grid h-full place-items-center">
                          <Play className="size-4 text-ink-3" aria-hidden="true" />
                        </span>
                      )}
                    </span>
                    <span className="min-w-0">
                      <span className="block truncate text-[12px] text-ink">{video.title}</span>
                      <span className="mt-0.5 block font-mono text-[9.5px] uppercase tracking-[0.1em] text-ink-3">
                        {index === 0 ? t("Featured") : t("Trailer")}
                      </span>
                    </span>
                  </button>
                ))}
              </div>
            )}
          </div>
        </section>
      )}

      <section aria-labelledby="media-heading">
        <SectionHeader title={t("Media")} count={items.length} description={t("Stills and videos")} />
        <div id="media-heading" className="nav-scroll -mx-[var(--page-x)] flex gap-3 overflow-x-auto px-[var(--page-x)] pb-2">
          {items.map((item, index) => (
            <button
              key={item.kind === "video" ? `video-${index}-${item.video.url}` : `still-${index}`}
              type="button"
              onClick={() => setLightbox(index)}
              className="group relative w-[240px] shrink-0 overflow-hidden rounded-[2px] border border-line text-left transition-colors hover:border-line-2 sm:w-[300px]"
              aria-label={
                item.kind === "video" ? `Open video ${index + 1}` : `Open still ${item.index + 1}`
              }
            >
              <MediaThumb item={item} game={game} spec={spec} seed={seed} />
              {item.kind === "video" && (
                <span className="absolute inset-0 grid place-items-center bg-stage/25">
                  <span className="media-control grid size-11 place-items-center rounded-full">
                    <Play className="ml-0.5 size-4" aria-hidden="true" />
                  </span>
                </span>
              )}
            </button>
          ))}
        </div>
      </section>

      <MediaLightbox
        open={lightbox !== null}
        onOpenChange={(open) => !open && setLightbox(null)}
        items={items}
        index={lightbox ?? 0}
        onIndexChange={setLightbox}
        game={game}
        spec={spec}
        seed={seed}
      />
    </>
  );
}
