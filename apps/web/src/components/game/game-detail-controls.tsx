import type { ReactNode } from "react";
import { MoreHorizontal } from "lucide-react";
import { cn } from "@/lib/cn";
import { stateLabel } from "@/lib/format";
import type { ProgressState } from "@/lib/types";
import { STATE_TONE } from "@/components/game/state";
import { IconButton, Skeleton } from "@/components/ui/controls";
import { DropdownMenu, useDropdown } from "@/components/ui/select";
import { t } from "@/lib/i18n";

const STATES: ProgressState[] = ["unplayed", "playing", "completed", "onHold", "abandoned", "planToPlay"];

export function DetailMenu({ items }: { items: { label: string; icon?: ReactNode; danger?: boolean; onSelect: () => void }[] }) {
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

export function StatePicker({ state, onChange }: { state: ProgressState; onChange: (state: ProgressState) => void }) {
  const menu = useDropdown();
  return (
    <>
      <button
        {...menu.triggerProps}
        className="flex h-9 items-center gap-2 rounded-[var(--r-sm)] border border-line bg-surface px-3 text-label text-ink-2 transition-colors hover:border-line-2 hover:text-ink"
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

export function DetailSkeleton() {
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
