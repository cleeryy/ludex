import { Link } from "react-router-dom";
import { Lock, Sparkles, Users } from "lucide-react";
import type { Collection } from "@/lib/types";
import { cn } from "@/lib/cn";
import { Badge } from "@/components/ui/controls";
import { t } from "@/lib/i18n";

export function CollectionCard({ collection, className }: { collection: Collection; className?: string }) {
  return (
    <Link
      to={`/collections/${collection.id}`}
      className={cn(
        "group flex min-h-[150px] flex-col justify-between gap-5 border border-line bg-surface/40 p-4 transition-colors hover:border-line-2 hover:bg-surface",
        className,
      )}
    >
      <div className="flex items-start justify-between gap-3">
        <div className="flex flex-wrap items-center gap-1.5">
          <Badge tone={collection.kind === "smart" ? "signal" : "default"}>
            {collection.kind === "smart" ? (
              <span className="flex items-center gap-1">
                <Sparkles className="size-2.5" aria-hidden="true" />
                {t("Smart")}
              </span>
            ) : (
              "Manual"
            )}
          </Badge>
          {collection.isPublic ? (
            <Badge tone="outline">
              <span className="flex items-center gap-1">
                <Users className="size-2.5" aria-hidden="true" />
                {t("Public")}
              </span>
            </Badge>
          ) : (
            <Badge tone="outline">
              <span className="flex items-center gap-1">
                <Lock className="size-2.5" aria-hidden="true" />
                {t("Private")}
              </span>
            </Badge>
          )}
        </div>
        <span className="num shrink-0 text-[11px] text-ink-3">
          {collection.gameCount} {collection.gameCount === 1 ? "game" : "games"}
        </span>
      </div>

      <div>
        <h3 className="text-[18px] font-semibold leading-tight tracking-[-0.015em] text-ink group-hover:text-ink">
          {collection.name}
        </h3>
        {collection.description ? (
          <p className="mt-1 line-clamp-2 text-[12.5px] leading-relaxed text-ink-3">{collection.description}</p>
        ) : (
          <p className="mt-1 text-[12.5px] text-ink-3">{collection.kind === "smart" ? t("Built from rules.") : t("An empty shelf.")}</p>
        )}
        <p className="mt-3 font-mono text-[10px] uppercase tracking-[0.1em] text-ink-3">
          {collection.owned ? t("Yours") : `By ${collection.owner.displayName ?? collection.owner.username}`}
        </p>
      </div>
    </Link>
  );
}
