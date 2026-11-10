import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Plus } from "lucide-react";
import { api } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { CollectionCard } from "@/components/collections/collection-card";
import { CollectionDialog } from "@/components/collections/collection-dialog";
import { EmptyState, ErrorState, PageHeader, SectionHeader } from "@/components/page";
import { Button, Skeleton } from "@/components/ui/controls";

export function CollectionsScreen() {
  const [createOpen, setCreateOpen] = useState(false);
  const { data, isPending, isError, error, refetch } = useQuery({
    queryKey: ["collections"],
    queryFn: () => api.listCollections(),
  });

  const own = (data ?? []).filter((collection) => collection.owned);
  const shared = (data ?? []).filter((collection) => !collection.owned);

  return (
    <div className="page page-in">
      <PageHeader
        title="Collections"
        meta={
          <span>
            {own.length} {own.length === 1 ? "shelf" : "shelves"} of yours
            {shared.length > 0 ? ` · ${shared.length} shared with you` : ""}
          </span>
        }
        description="Shelves you build by hand, and shelves that keep themselves up to date from rules."
        actions={
          <Button variant="primary" icon={<Plus className="size-4" />} onClick={() => setCreateOpen(true)}>
            New collection
          </Button>
        }
      />

      {isPending && (
        <div className="mt-8 grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
          {Array.from({ length: 3 }).map((_, index) => (
            <Skeleton key={index} className="h-[150px] w-full" />
          ))}
        </div>
      )}

      {isError && (
        <div className="mt-8">
          <ErrorState title="Collections didn’t load" message={errorMessage(error)} onRetry={() => void refetch()} />
        </div>
      )}

      {!isPending && !isError && own.length === 0 && shared.length === 0 && (
        <div className="mt-8">
          <EmptyState
            title="No shelves yet"
            description="A collection is a shelf: an explicit list for a co-op night, or a rule that gathers everything you have not finished. Public shelves show up on your community profile."
          >
            <Button variant="primary" icon={<Plus className="size-4" />} onClick={() => setCreateOpen(true)}>
              Create the first one
            </Button>
          </EmptyState>
        </div>
      )}

      {own.length > 0 && (
        <section className="mt-10" aria-labelledby="own-collections">
          <SectionHeader title="Yours" count={own.length} />
          <div id="own-collections" className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
            {own.map((collection) => (
              <CollectionCard key={collection.id} collection={collection} />
            ))}
          </div>
        </section>
      )}

      {shared.length > 0 && (
        <section className="mt-12" aria-labelledby="shared-collections">
          <SectionHeader title="Shared with you" count={shared.length} description="Public shelves from other accounts" />
          <div id="shared-collections" className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
            {shared.map((collection) => (
              <CollectionCard key={collection.id} collection={collection} />
            ))}
          </div>
        </section>
      )}

      <CollectionDialog open={createOpen} onOpenChange={setCreateOpen} />
    </div>
  );
}
