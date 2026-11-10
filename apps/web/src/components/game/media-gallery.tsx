import { useCallback, useEffect, useRef, useState } from "react";
import { ChevronLeft, ChevronRight, Maximize2, Minimize2, Play } from "lucide-react";
import { useReducedMotion } from "motion/react";
import { assetUrl } from "@/lib/api";
import { cn } from "@/lib/cn";
import type { ArtSpec } from "@/lib/art";
import type { GameDetail, GameVideo } from "@/lib/types";
import { StillPlate } from "@/components/art/cover";
import { Button, IconButton } from "@/components/ui/controls";
import { Dialog } from "@/components/ui/dialog";
import { SectionHeader } from "@/components/page";

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
      <div className={cn("media-plate aspect-video w-full overflow-hidden bg-black", className)}>
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
            className="h-full w-full bg-black"
          />
        )}
      </div>
    );
  }

  return (
    <button
      type="button"
      onClick={() => setPlaying(true)}
      className={cn("media-plate group relative block aspect-video w-full overflow-hidden bg-bg-deep text-left", className)}
      aria-label={`Play trailer: ${video.title || title}`}
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
      <span className="absolute inset-0 bg-[linear-gradient(180deg,rgba(5,5,5,0.05),rgba(5,5,5,0.62))]" aria-hidden="true" />
      <span className="absolute inset-0 grid place-items-center">
        <span
          className={cn(
            "grid size-16 place-items-center rounded-full border border-white/35 bg-black/55 text-white backdrop-blur-sm transition-transform duration-[180ms]",
            !reduce && "group-hover:scale-105",
          )}
        >
          <Play className="ml-0.5 size-6" aria-hidden="true" />
        </span>
      </span>
      <span className="absolute inset-x-0 bottom-0 flex items-end justify-between gap-4 p-4">
        <span className="min-w-0">
          <span className="block truncate text-[14px] font-medium text-white">{video.title || title}</span>
          <span className="mt-0.5 block font-mono text-[10px] uppercase tracking-[0.14em] text-white/65">
            {isYoutube ? "YouTube · no cookies until play" : "Video"}
          </span>
        </span>
        <span className="hidden shrink-0 border border-white/25 px-2 py-1 font-mono text-[10px] uppercase tracking-[0.12em] text-white/70 sm:block">
          Play
        </span>
      </span>
    </button>
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
    <Dialog open={open} onOpenChange={onOpenChange} title={`${game.title} — media`} size="lg">
      <div className="flex flex-col gap-3">
        <div ref={stageRef} className="bg-bg-deep">
          <MediaFrame item={current} game={game} spec={spec} seed={seed} />
        </div>
        <div className="flex flex-wrap items-center justify-between gap-3">
          <span className="font-mono text-[11px] text-ink-3">
            {index + 1} / {items.length}
            {current.kind === "video" ? " · video" : " · still"}
          </span>
          <div className="flex flex-wrap gap-2">
            <Button
              variant="default"
              size="sm"
              icon={<ChevronLeft className="size-3.5" />}
              onClick={() => onIndexChange((index - 1 + items.length) % items.length)}
            >
              Previous
            </Button>
            <Button
              variant="default"
              size="sm"
              iconRight={<ChevronRight className="size-3.5" />}
              onClick={() => onIndexChange((index + 1) % items.length)}
            >
              Next
            </Button>
            <IconButton
              label={fullscreen ? "Leave fullscreen" : "Fullscreen"}
              variant="default"
              onClick={() => void toggleFullscreen()}
            >
              {fullscreen ? <Minimize2 className="size-4" /> : <Maximize2 className="size-4" />}
            </IconButton>
          </div>
        </div>
        <p className="text-[11.5px] text-ink-3">
          Arrow keys move through the gallery, Escape closes it. Trailers only load after you press play.
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
            title="Trailer"
            count={videos.length}
            description={videos.length > 1 ? "Choose a video, then press play" : undefined}
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
                        {index === 0 ? "Featured" : "Trailer"}
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
        <SectionHeader title="Media" count={items.length} description="Stills and videos" />
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
              <MediaFrame item={item} game={game} spec={spec} seed={seed} />
              {item.kind === "video" && (
                <span className="absolute inset-0 grid place-items-center bg-black/25">
                  <span className="grid size-11 place-items-center rounded-full border border-white/30 bg-black/50 text-white">
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
