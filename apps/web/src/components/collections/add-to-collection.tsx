import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Check, ListPlus, Plus } from "lucide-react";
import { api } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { useToast } from "@/state/toasts";
import { Button, Input } from "@/components/ui/controls";
import { Dialog } from "@/components/ui/dialog";

/** Adds one game to an existing manual collection, or creates a new one. */
export function AddToCollectionDialog({
  open,
  onOpenChange,
  gameId,
  gameTitle,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  gameId: number;
  gameTitle: string;
}) {
  const queryClient = useQueryClient();
  const { push } = useToast();
  const [busyId, setBusyId] = useState<number | null>(null);
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState("");

  const collections = useQuery({
    queryKey: ["collections"],
    queryFn: () => api.listCollections(),
    enabled: open,
  });
  const own = (collections.data ?? []).filter((collection) => collection.owned && collection.kind === "manual");

  const add = async (collectionId: number, collectionName: string) => {
    setBusyId(collectionId);
    try {
      const result = await api.addCollectionGames(collectionId, [gameId]);
      await queryClient.invalidateQueries({ queryKey: ["collections"] });
      await queryClient.invalidateQueries({ queryKey: ["collection", collectionId] });
      push({
        title: result.added ? "Added to collection" : "Already in that collection",
        description: result.added ? `${gameTitle} is now in ${collectionName}.` : `${gameTitle} was already there.`,
        tone: result.added ? "success" : "default",
      });
      onOpenChange(false);
    } catch (cause) {
      push({ title: "Could not add the game", description: errorMessage(cause), tone: "danger" });
    } finally {
      setBusyId(null);
    }
  };

  const createAndAdd = async () => {
    const trimmed = name.trim();
    if (!trimmed) return;
    setCreating(true);
    try {
      const collection = await api.createCollection({ name: trimmed, gameIds: [gameId] });
      await queryClient.invalidateQueries({ queryKey: ["collections"] });
      push({ title: "Collection created", description: `${gameTitle} is the first game in ${collection.name}.`, tone: "success" });
      setName("");
      onOpenChange(false);
    } catch (cause) {
      push({ title: "Could not create the collection", description: errorMessage(cause), tone: "danger" });
    } finally {
      setCreating(false);
    }
  };

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title="Add to a collection"
      description={`${gameTitle} can live in as many shelves as you like.`}
      size="sm"
    >
      <div className="flex flex-col gap-5">
        <div>
          <p className="pb-2 text-[12px] text-ink-3">Your manual collections</p>
          {collections.isPending ? (
            <p className="text-[13px] text-ink-3">Loading…</p>
          ) : own.length === 0 ? (
            <p className="border border-line px-3 py-3 text-[13px] leading-relaxed text-ink-2">
              No manual collections yet. Create one below and this game becomes its first entry.
            </p>
          ) : (
            <div className="border-t border-line">
              {own.map((collection) => (
                <button
                  key={collection.id}
                  type="button"
                  onClick={() => void add(collection.id, collection.name)}
                  disabled={busyId !== null}
                  className="flex w-full items-center gap-3 border-b border-line py-3 text-left transition-colors hover:bg-surface-2 disabled:opacity-50"
                >
                  <span className="grid size-8 shrink-0 place-items-center border border-line text-ink-3">
                    <ListPlus className="size-3.5" aria-hidden="true" />
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="block truncate text-[13.5px] text-ink">{collection.name}</span>
                    <span className="mt-0.5 block font-mono text-[10px] text-ink-3">
                      {collection.gameCount} {collection.gameCount === 1 ? "game" : "games"}
                      {collection.isPublic ? " · public" : ""}
                    </span>
                  </span>
                  {busyId === collection.id ? (
                    <span className="text-[11.5px] text-ink-3">Adding…</span>
                  ) : (
                    <Plus className="size-4 shrink-0 text-ink-3" aria-hidden="true" />
                  )}
                </button>
              ))}
            </div>
          )}
        </div>

        <div>
          <p className="pb-2 text-[12px] text-ink-3">New collection</p>
          <div className="flex items-stretch gap-2">
            <Input
              value={name}
              onChange={(event) => setName(event.target.value)}
              placeholder="Friday night co-op"
              aria-label="New collection name"
              onKeyDown={(event) => {
                if (event.key === "Enter") void createAndAdd();
              }}
            />
            <Button
              variant="default"
              loading={creating}
              disabled={!name.trim()}
              icon={<Check className="size-4" />}
              onClick={() => void createAndAdd()}
            >
              Create
            </Button>
          </div>
        </div>
      </div>
    </Dialog>
  );
}
