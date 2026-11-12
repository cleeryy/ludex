import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { Copy, Link2, Lock } from "lucide-react";
import { api, assetUrl } from "@/lib/api";
import { getCollectionShare, unlockCollectionShare } from "@/lib/collection-shares";
import { ApiError, errorMessage } from "@/lib/http";
import { formatDate, formatYear } from "@/lib/format";
import type { CollectionShareView } from "@/lib/types";
import { useAuth } from "@/state/auth";
import { useToast } from "@/state/toasts";
import { GameCover } from "@/components/art/cover";
import { Wordmark } from "@/components/brand";
import { SectionHeader } from "@/components/page";
import { PublicPage } from "@/components/public-page";
import { WindowControls, useWindowChrome } from "@/components/shell/window-controls";
import { Badge, Button, Field, Input, Skeleton } from "@/components/ui/controls";
import { Dialog } from "@/components/ui/dialog";
import { t, tn } from "@/lib/i18n";

/**
 * Public collection page (#173): reachable logged out at
 * `#/collection-share/{token}`. It renders only the metadata the server puts in
 * `CollectionShareView` — never account data, file paths or download links.
 */
export function CollectionShareScreen() {
  const { token } = useParams<{ token: string }>();
  const shareToken = token ?? "";
  const { user } = useAuth();
  const { push } = useToast();
  const navigate = useNavigate();
  const [unlocked, setUnlocked] = useState<CollectionShareView | null>(null);
  const [password, setPassword] = useState("");
  const [unlocking, setUnlocking] = useState(false);
  const [unlockError, setUnlockError] = useState<string | null>(null);
  const [copyOpen, setCopyOpen] = useState(false);
  const [copying, setCopying] = useState(false);

  const view = useQuery({
    queryKey: ["public-collection-share", shareToken],
    queryFn: () => getCollectionShare(shareToken),
    enabled: shareToken.length > 0,
    retry: false,
  });

  const data = unlocked ?? view.data;

  useEffect(() => {
    if (!data) return;
    const name = data.collection.name;
    document.title = `${name} · Ludex`;
    const setMeta = (property: string, content: string) => {
      let tag = document.querySelector<HTMLMetaElement>(`meta[property="${property}"]`);
      if (!tag) {
        tag = document.createElement("meta");
        tag.setAttribute("property", property);
        document.head.appendChild(tag);
      }
      tag.content = content;
    };
    setMeta("og:title", name);
    setMeta("og:description", data.collection.description ?? t("A shared collection on Ludex"));
    const cover = data.games.find((game) => game.coverUrl)?.coverUrl;
    if (cover) setMeta("og:image", assetUrl(cover) ?? "");
    return () => {
      document.title = "Ludex";
    };
  }, [data]);

  const unlock = async () => {
    setUnlocking(true);
    setUnlockError(null);
    try {
      setUnlocked(await unlockCollectionShare(shareToken, password));
    } catch (cause) {
      setUnlockError(cause instanceof ApiError ? cause.message : errorMessage(cause));
    } finally {
      setUnlocking(false);
    }
  };

  const copy = async () => {
    if (!data) return;
    setCopying(true);
    try {
      const result = await api.importCollection({
        token: shareToken,
        password: data.hasPassword ? password : undefined,
      });
      push({
        title: result.created ? t("Collection copied") : t("Collection is already in your library"),
        description: t("“{collection}” is now one of your private shelves.", { collection: result.collection.name }),
        tone: "success",
      });
      setCopyOpen(false);
      navigate(`/collections/${result.collection.id}`);
    } catch (cause) {
      push({ title: t("Could not copy the collection"), description: errorMessage(cause), tone: "danger" });
      setCopying(false);
    }
  };

  if (!shareToken || (view.isPending && !unlocked)) return <ShareLoading />;
  if ((view.isError || !view.data) && !unlocked) {
    return <ShareUnavailable error={view.error} onRetry={() => void view.refetch()} />;
  }
  if (!data) return <ShareLoading />;

  const { collection, games } = data;
  const gamesHidden = data.hasPassword && !unlocked;

  return (
    <PublicPage>
      <PublicHeader marker />

      <main className="page pb-20 pt-6">
        <div className="max-w-[760px]">
          <p className="font-mono text-[10px] uppercase tracking-[0.16em] text-ink-3">{t("Shared collection")}</p>
          <h1 className="mt-3 text-[clamp(2.2rem,4.6vw,3.8rem)] font-semibold leading-[0.96] tracking-[-0.033em]">
            {collection.name}
          </h1>
          <div className="mt-3 flex flex-wrap items-center gap-x-4 gap-y-2 text-[12.5px] text-ink-2">
            {collection.owner && <span>{t("By {name}", { name: collection.owner.displayName })}</span>}
            <span>{tn(collection.gameCount, "1 game", "{count} games")}</span>
            <Badge tone={collection.kind === "smart" ? "signal" : "default"}>
              {collection.kind === "smart" ? t("Smart list") : t("Manual shelf")}
            </Badge>
            {data.expiresAt && <span className="text-ink-3">{t("Link expires {date}", { date: formatDate(data.expiresAt) })}</span>}
          </div>
          {collection.description && (
            <p className="measure mt-4 text-[14.5px] leading-relaxed text-ink-2">{collection.description}</p>
          )}

          <div className="mt-6 flex flex-wrap items-center gap-3">
            {gamesHidden ? (
              <form
                className="flex w-full max-w-[440px] flex-col gap-2 sm:flex-row sm:items-end"
                onSubmit={(event) => {
                  event.preventDefault();
                  void unlock();
                }}
              >
                <Field label={t("Password")} htmlFor="collection-share-password" hint={t("Set by the person who shared this collection.")}>
                  <Input
                    id="collection-share-password"
                    type="password"
                    autoComplete="off"
                    required
                    value={password}
                    onChange={(event) => setPassword(event.target.value)}
                    placeholder={t("Enter the share password")}
                    disabled={unlocking}
                  />
                </Field>
                <Button type="submit" variant="primary" loading={unlocking} icon={<Lock className="size-4" />}>
                  {t("Unlock")}
                </Button>
              </form>
            ) : (
              <Button variant="primary" icon={<Copy className="size-4" />} onClick={() => setCopyOpen(true)}>
                {t("Copy to my collections")}
              </Button>
            )}
            {!user && (
              <p className="text-[11.5px] leading-relaxed text-ink-3">
                {t("Sign in to copy this collection into your library. Viewing never needs an account.")}
              </p>
            )}
          </div>
          {unlockError && (
            <p role="alert" className="mt-3 border-l-2 border-danger pl-3 text-[12.5px] text-danger">
              {unlockError}
            </p>
          )}
        </div>

        <section className="mt-12" aria-labelledby="shared-games">
          <SectionHeader
            title={t("Games")}
            count={gamesHidden ? 0 : games.length}
            description={gamesHidden ? t("Unlock the link to see the games") : undefined}
          />
          {gamesHidden ? (
            <div className="flex items-center gap-3 border border-line bg-surface/40 p-5 text-[12.5px] text-ink-3">
              <Lock className="size-4 shrink-0" aria-hidden="true" />
              {t("This link is password-protected. The game list stays hidden until the password is entered.")}
            </div>
          ) : games.length === 0 ? (
            <p className="border border-line bg-surface/40 p-5 text-[12.5px] text-ink-3">
              {t("This shelf is empty right now.")}
            </p>
          ) : (
            <div id="shared-games" className="grid grid-cols-2 gap-x-4 gap-y-8 sm:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5">
              {games.map((game) => (
                <div key={game.id}>
                  <div className="shadow-plate">
                    <GameCover
                      title={game.title}
                      coverUrl={game.coverUrl}
                      index={game.id}
                      year={formatYear(game.releaseYear ? `${game.releaseYear}-01-01` : null)}
                      genre={game.genres[0]}
                    />
                  </div>
                  <p className="mt-2 line-clamp-2 text-[13px] text-ink">{game.title}</p>
                  <p className="mt-0.5 font-mono text-[10px] text-ink-3">
                    {formatYear(game.releaseYear ? `${game.releaseYear}-01-01` : null)}
                    {game.genres.length > 0 ? ` · ${game.genres.slice(0, 2).join(" / ")}` : ""}
                  </p>
                </div>
              ))}
            </div>
          )}
        </section>

        <p className="mt-12 max-w-[70ch] border-t border-line pt-4 text-[11.5px] leading-relaxed text-ink-3">
          {t("Anyone with this link can see the collection name, description, game titles and cover art. No files, downloads, file paths or account data are shared. The owner can revoke the link or delete the collection at any time.")}
        </p>
      </main>

      <Dialog
        open={copyOpen}
        onOpenChange={setCopyOpen}
        title={t("Copy to my collections")}
        description={t("A private copy is created in your account. Game files are never copied or downloaded.")}
        footer={
          <>
            <Button variant="ghost" onClick={() => setCopyOpen(false)}>
              {t("Cancel")}
            </Button>
            {user ? (
              <Button variant="primary" loading={copying} onClick={() => void copy()}>
                {t("Copy {count} games", { count: collection.gameCount })}
              </Button>
            ) : (
              <Button variant="primary" onClick={() => window.location.assign("/#/login")}>
                {t("Sign in to copy")}
              </Button>
            )}
          </>
        }
      >
        <div className="border border-line px-3 py-2.5">
          <p className="text-[13px] text-ink">{collection.name}</p>
          <p className="mt-0.5 text-[11.5px] text-ink-3">{tn(collection.gameCount, "1 game", "{count} games")}</p>
        </div>
        <p className="mt-3 text-[11.5px] leading-relaxed text-ink-3">
          {t("Importing the same collection twice merges missing games into the existing copy instead of creating a duplicate.")}
        </p>
      </Dialog>
    </PublicPage>
  );
}

function ShareLoading() {
  return (
    <PublicPage>
      <PublicHeader />
      <div className="page mt-8 flex flex-col gap-4">
        <Skeleton className="h-3 w-40" />
        <Skeleton className="h-12 w-3/4" />
        <Skeleton className="h-3 w-56" />
        <div className="mt-8 grid grid-cols-2 gap-4 sm:grid-cols-4">
          {Array.from({ length: 4 }).map((_, index) => (
            <Skeleton key={index} className="aspect-[3/4] w-full" />
          ))}
        </div>
      </div>
    </PublicPage>
  );
}

function ShareUnavailable({ error, onRetry }: { error: unknown; onRetry: () => void }) {
  const apiError = error instanceof ApiError ? error : null;
  const notFound = apiError?.status === 404;
  const rateLimited = apiError?.status === 429;
  return (
    <PublicPage>
      <PublicHeader marker />
      <main className="page flex items-center pb-24">
        <div className="max-w-[520px]">
          <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-ink-3">{t("Shared collection")}</p>
          <h1 className="mt-3 text-[clamp(2.2rem,4.4vw,3.4rem)] font-semibold leading-[0.96] tracking-[-0.033em]">
            {notFound ? t("This link is not available") : rateLimited ? t("Too many attempts") : t("The link didn’t load")}
          </h1>
          <p className="mt-4 border-l-2 border-danger pl-3 text-[14px] leading-relaxed text-ink-2" role="alert">
            {apiError?.message ?? errorMessage(error)}
          </p>
          <p className="mt-4 max-w-[46ch] text-[12.5px] leading-relaxed text-ink-3">
            {t("Collection links can be revoked, expire, or stop working when the owner makes the shelf private. Ask the person who sent it for a new link.")}
          </p>
          {!notFound && (
            <Button variant="default" className="mt-6" onClick={onRetry}>
              {t("Try again")}
            </Button>
          )}
        </div>
      </main>
    </PublicPage>
  );
}

function PublicHeader({ marker = false }: { marker?: boolean }) {
  const chrome = useWindowChrome();
  return (
    <header
      data-tauri-drag-region={chrome.custom ? "deep" : undefined}
      className="page flex flex-wrap items-center justify-between gap-3 pb-5 pt-[max(1.25rem,env(safe-area-inset-top))]"
    >
      <Wordmark height={18} />
      <div className="flex items-center gap-3">
        {marker && (
          <span className="flex items-center gap-2 font-mono text-[10px] uppercase tracking-[0.16em] text-ink-3">
            <Link2 className="size-3.5" aria-hidden="true" />
            {t("Shared link")}
          </span>
        )}
        <WindowControls chrome={chrome} />
      </div>
    </header>
  );
}
