import { useMemo, useState } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, Pencil, Plus, Sparkles, Trash2, X } from "lucide-react";
import { api } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { formatYear } from "@/lib/format";
import { rulesSummary, ruleLabel } from "@/lib/collections";
import type { Collection } from "@/lib/types";
import { useAllGames, useBookmark, useInstalledMap, useLaunchGame } from "@/lib/hooks";
import { isNative } from "@/lib/native";
import { useToast } from "@/state/toasts";
import { GameCard } from "@/components/game/game-card";
import { CollectionDialog } from "@/components/collections/collection-dialog";
import { EmptyState, ErrorState, PageHeader, SectionHeader } from "@/components/page";
import { Badge, Button, Checkbox, Input, Skeleton } from "@/components/ui/controls";
import { ConfirmDialog, Dialog } from "@/components/ui/dialog";
import { t, tn } from "@/lib/i18n";

export function CollectionScreen() {
  const { id } = useParams();
  const collectionId = Number(id);
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { push } = useToast();
  const installed = useInstalledMap();
  const bookmark = useBookmark();
  const launch = useLaunchGame();
  const [editOpen, setEditOpen] = useState(false);
  const [addOpen, setAddOpen] = useState(false);
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [deleting, setDeleting] = useState(false);

  const collection = useQuery({
    queryKey: ["collection", collectionId],
    queryFn: () => api.getCollection(collectionId),
    enabled: Number.isFinite(collectionId),
  });
  const games = useQuery({
    queryKey: ["collection-games", collectionId],
    queryFn: () => api.collectionGames(collectionId, 1, 200),
    enabled: Number.isFinite(collectionId),
  });

  const remove = async (gameId: number, title: string) => {
    try {
      await api.removeCollectionGame(collectionId, gameId);
      void queryClient.invalidateQueries({ queryKey: ["collection-games", collectionId] });
      void queryClient.invalidateQueries({ queryKey: ["collection", collectionId] });
      void queryClient.invalidateQueries({ queryKey: ["collections"] });
      push({ title: t("Removed from the shelf"), description: title });
    } catch (cause) {
      push({ title: t("Could not remove the game"), description: errorMessage(cause), tone: "danger" });
    }
  };

  const deleteCollection = async () => {
    setDeleting(true);
    try {
      await api.deleteCollection(collectionId);
      void queryClient.invalidateQueries({ queryKey: ["collections"] });
      push({ title: t("Collection deleted"), tone: "success" });
      navigate("/collections");
    } catch (cause) {
      push({ title: t("Could not delete the collection"), description: errorMessage(cause), tone: "danger" });
      setDeleting(false);
    }
  };

  if (collection.isPending) {
    return (
      <div className="page page-in pt-8">
        <Skeleton className="h-3 w-20" />
        <Skeleton className="mt-6 h-12 w-2/3" />
        <Skeleton className="mt-3 h-3 w-1/3" />
        <div className="mt-10 grid grid-cols-2 gap-x-4 gap-y-8 sm:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5">
          {Array.from({ length: 5 }).map((_, index) => (
            <Skeleton key={index} className="aspect-[3/4] w-full" />
          ))}
        </div>
      </div>
    );
  }

  if (collection.isError || !collection.data) {
    const notFound =
      collection.error instanceof Error && collection.error.message.toLowerCase().includes("not found");
    return (
      <div className="page page-in pt-8">
        <ErrorState
          title={notFound ? t("Collection not found") : t("That shelf didn’t load")}
          message={
            notFound
              ? t("It is private, or it no longer exists. Ask the owner to publish it if you expected to see it.")
              : errorMessage(collection.error)
          }
          onRetry={() => void collection.refetch()}
          action={
            <Button variant="ghost" onClick={() => navigate("/collections")}>
              {t("All collections")}
            </Button>
          }
        />
      </div>
    );
  }

  const data = collection.data;
  const items = games.data?.items ?? [];
  const isOwner = data.owned;

  return (
    <div className="page page-in">
      <Link
        to="/collections"
        className="inline-flex h-8 items-center gap-2 text-[12.5px] text-ink-2 transition-colors hover:text-ink"
      >
        <ArrowLeft className="rtl:-scale-x-100 size-3.5" aria-hidden="true" />
        {t("Collections")}
      </Link>

      <PageHeader
        className="mt-4"
        title={data.name}
        meta={
          <>
            <Badge tone={data.kind === "smart" ? "signal" : "default"}>
              {data.kind === "smart" ? t("Smart") : t("Manual")}
            </Badge>
            <Badge tone="outline">{data.isPublic ? t("Public") : t("Private")}</Badge>
            <span>
              {data.gameCount} {data.gameCount === 1 ? "game" : "games"}
            </span>
            <span>
              {isOwner ? t("Yours") : `By ${data.owner.displayName ?? data.owner.username}`}
            </span>
          </>
        }
        description={data.description ?? undefined}
        actions={
          isOwner ? (
            <>
              {data.kind === "manual" && (
                <Button variant="default" icon={<Plus className="size-4" />} onClick={() => setAddOpen(true)}>
                  {t("Add games")}
                </Button>
              )}
              <Button variant="default" icon={<Pencil className="size-4" />} onClick={() => setEditOpen(true)}>
                {t("Edit")}
              </Button>
              <Button
                variant="ghost"
                className="text-ink-3 hover:text-danger"
                icon={<Trash2 className="size-4" />}
                onClick={() => setDeleteOpen(true)}
              >
                {t("Delete")}
              </Button>
            </>
          ) : (
            <Link
              to={`/community/u/${data.owner.id}`}
              className="text-[12.5px] text-ink-3 underline-offset-4 hover:text-ink hover:underline"
            >
              {t("{name}’s profile", { name: data.owner.displayName ?? data.owner.username })}
            </Link>
          )
        }
      />

      {data.kind === "smart" && (
        <section className="mt-8 border border-line bg-surface/40 p-4" aria-labelledby="rules-heading">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div className="flex items-center gap-2">
              <Sparkles className="size-3.5 text-signal-text" aria-hidden="true" />
              <h2 id="rules-heading" className="text-[13px] font-medium text-ink">
                {t("Rules")}
              </h2>
              <span className="text-[11.5px] text-ink-3">
                {data.rules.match === "all" ? t("All must match") : t("Any can match")}
              </span>
            </div>
            {isOwner && (
              <Button variant="ghost" size="sm" onClick={() => setEditOpen(true)}>
                {t("Edit rules")}
              </Button>
            )}
          </div>
          <p className="mt-2 text-[13px] leading-relaxed text-ink-2">{rulesSummary(data.rules)}</p>
          {data.rules.rules.length > 0 && (
            <div className="mt-3 flex flex-wrap gap-1.5">
              {data.rules.rules.map((rule, index) => (
                <span key={`${rule.field}-${index}`} className="border border-line px-2 py-1 font-mono text-[10.5px] text-ink-3">
                  {ruleLabel(rule)}
                </span>
              ))}
            </div>
          )}
          <p className="mt-3 text-[11px] leading-relaxed text-ink-3">
            {t("Playtime and availability are evaluated for the account viewing the shelf. Empty rules match nothing.")}
          </p>
        </section>
      )}

      <section className="mt-10" aria-labelledby="collection-games">
        <SectionHeader
          title={data.kind === "smart" ? t("Matching games") : t("Games")}
          count={items.length}
          description={data.kind === "smart" ? t("Evaluated live") : undefined}
        />

        {games.isPending && (
          <div id="collection-games" className="grid grid-cols-2 gap-x-4 gap-y-8 sm:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5">
            {Array.from({ length: 5 }).map((_, index) => (
              <Skeleton key={index} className="aspect-[3/4] w-full" />
            ))}
          </div>
        )}

        {games.isError && (
          <ErrorState title={t("The games didn’t load")} message={errorMessage(games.error)} onRetry={() => void games.refetch()} />
        )}

        {!games.isPending && !games.isError && items.length === 0 && (
          <EmptyState
            compact
            title={data.kind === "smart" ? t("Nothing matches yet") : t("This shelf is empty")}
            description={
              data.kind === "smart"
                ? t("No game in the library satisfies every rule right now. Loosen one, or add more games.")
                : isOwner
                  ? t("Add games by hand and they will appear here.")
                  : t("The owner has not added anything yet.")
            }
          >
            {isOwner && data.kind === "manual" && (
              <Button variant="default" icon={<Plus className="size-4" />} onClick={() => setAddOpen(true)}>
                {t("Add games")}
              </Button>
            )}
            {isOwner && data.kind === "smart" && (
              <Button variant="default" onClick={() => setEditOpen(true)}>
                {t("Edit rules")}
              </Button>
            )}
          </EmptyState>
        )}

        {items.length > 0 && (
          <div id="collection-games" className="grid grid-cols-2 gap-x-4 gap-y-8 sm:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5">
            {items.map((game) => (
              <div key={game.id} className="relative">
                <GameCard
                  game={game}
                  index={game.id}
                  installed={installed.get(game.id)}
                  onBookmark={() => bookmark.mutate({ id: game.id, bookmarked: !game.bookmarked })}
                  onPlay={
                    isNative() && installed.has(game.id)
                      ? () => launch.mutate({ id: game.id, title: game.title, executable: installed.get(game.id)?.executable })
                      : undefined
                  }
                />
                {isOwner && data.kind === "manual" && (
                  <button
                    type="button"
                    onClick={() => void remove(game.id, game.title)}
                    aria-label={t("Remove {title} from this collection", { title: game.title })}
                    className="absolute right-0 top-0 z-[6] grid size-8 place-items-center rounded-[2px] border border-line bg-bg/85 text-ink-3 backdrop-blur-sm transition-colors hover:border-danger/60 hover:text-danger"
                  >
                    <X className="size-3.5" />
                  </button>
                )}
              </div>
            ))}
          </div>
        )}
      </section>

      <CollectionDialog open={editOpen} onOpenChange={setEditOpen} collection={data} />
      <AddGamesDialog open={addOpen} onOpenChange={setAddOpen} collection={data} existing={items.map((game) => game.id)} />
      <ConfirmDialog
        open={deleteOpen}
        onOpenChange={setDeleteOpen}
        title={t("Delete this collection?")}
        description={t("“{collection}” will be removed. The games stay in the library.", { collection: data.name })}
        confirmLabel={t("Delete collection")}
        loading={deleting}
        onConfirm={() => void deleteCollection()}
      />
    </div>
  );
}

function AddGamesDialog({
  open,
  onOpenChange,
  collection,
  existing,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  collection: Collection;
  existing: number[];
}) {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const { data } = useAllGames();
  const [selected, setSelected] = useState<number[]>([]);
  const [search, setSearch] = useState("");
  const [saving, setSaving] = useState(false);

  const candidates = useMemo(() => {
    const query = search.trim().toLowerCase();
    const taken = new Set(existing);
    return (data?.items ?? []).filter(
      (game) => !taken.has(game.id) && (!query || game.title.toLowerCase().includes(query)),
    );
  }, [data, existing, search]);

  const save = async () => {
    if (!selected.length) {
      onOpenChange(false);
      return;
    }
    setSaving(true);
    try {
      const result = await api.addCollectionGames(collection.id, selected);
      void queryClient.invalidateQueries({ queryKey: ["collection-games", collection.id] });
      void queryClient.invalidateQueries({ queryKey: ["collection", collection.id] });
      void queryClient.invalidateQueries({ queryKey: ["collections"] });
      push({
        title: tn(result.added, "1 game added", "{count} games added"),
        description: t("{collection} now holds {total}.", { collection: collection.name, total: result.total }),
        tone: "success",
      });
      setSelected([]);
      setSearch("");
      onOpenChange(false);
    } catch (cause) {
      push({ title: t("Could not add the games"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setSaving(false);
    }
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) {
          setSelected([]);
          setSearch("");
        }
        onOpenChange(next);
      }}
      title={t("Add games")}
      description={t("Pick from the library. Games already on this shelf are hidden.")}
      footer={
        <>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            {t("Cancel")}
          </Button>
          <Button variant="primary" loading={saving} disabled={!selected.length} onClick={() => void save()}>
            {t("Add")} {selected.length || ""}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-2">
        <Input
          value={search}
          onChange={(event) => setSearch(event.target.value)}
          placeholder={t("Filter the library")}
          aria-label={t("Filter games")}
          className="h-9 text-[13px]"
        />
        <div className="max-h-[360px] overflow-y-auto border border-line">
          {candidates.length === 0 && <p className="p-4 text-[12.5px] text-ink-3">{t("Nothing left to add.")}</p>}
          {candidates.map((game) => {
            const checked = selected.includes(game.id);
            return (
              <button
                key={game.id}
                type="button"
                onClick={() =>
                  setSelected((current) =>
                    checked ? current.filter((id) => id !== game.id) : [...current, game.id],
                  )
                }
                aria-pressed={checked}
                className="flex w-full items-center gap-3 border-b border-line px-3 py-2 text-left last:border-0 hover:bg-surface-2"
              >
                <Checkbox checked={checked} onCheckedChange={() => undefined} label={t("Select {title}", { title: game.title })} />
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-[13px] text-ink">{game.title}</span>
                  <span className="font-mono text-[10px] text-ink-3">
                    {formatYear(game.releaseDate)} · {game.genres.slice(0, 2).join(" / ") || t("Unfiled")}
                  </span>
                </span>
              </button>
            );
          })}
        </div>
      </div>
    </Dialog>
  );
}
