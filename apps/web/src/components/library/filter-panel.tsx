import { useEffect, useState } from "react";
import { FilterX } from "lucide-react";
import { installsAvailable } from "@/lib/install";
import {
  PLAYER_OPTIONS,
  PROVIDER_OPTIONS,
  RATING_OPTIONS,
  SORT_LABELS,
  STATE_LABELS,
  type FilterKey,
  type LibraryFilters,
} from "@/lib/library-filters";
import { Badge, Button, Checkbox, Input, Segmented } from "@/components/ui/controls";
import { Dialog } from "@/components/ui/dialog";
import { Select } from "@/components/ui/select";

const STATE_OPTIONS = [
  { value: "any", label: "Any state" },
  ...Object.entries(STATE_LABELS).map(([value, label]) => ({ value, label })),
];

const SORT_OPTIONS = Object.entries(SORT_LABELS).map(([value, label]) => ({ value, label }));

/**
 * Filter sheet shared by desktop and mobile. Every control writes straight to
 * the URL through `onChange`; nothing is applied until the query string
 * changes, so the panel has no draft state to lose.
 */
export function FilterPanel({
  open,
  onOpenChange,
  filters,
  activeCount,
  onChange,
  onClearAll,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  filters: LibraryFilters;
  activeCount: number;
  onChange: (key: FilterKey, value: string | boolean) => void;
  onClearAll: () => void;
}) {
  const [yearFrom, setYearFrom] = useState(filters.yearFrom);
  const [yearTo, setYearTo] = useState(filters.yearTo);
  const [playtimeMin, setPlaytimeMin] = useState(filters.playtimeMin);
  const [playtimeMax, setPlaytimeMax] = useState(filters.playtimeMax);

  useEffect(() => setYearFrom(filters.yearFrom), [filters.yearFrom]);
  useEffect(() => setYearTo(filters.yearTo), [filters.yearTo]);
  useEffect(() => setPlaytimeMin(filters.playtimeMin), [filters.playtimeMin]);
  useEffect(() => setPlaytimeMax(filters.playtimeMax), [filters.playtimeMax]);

  const commitNumber = (key: FilterKey, value: string, setter: (next: string) => void) => {
    setter(value);
    onChange(key, value);
  };

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title="Filters"
      description="Narrow the shelf. Every filter is saved in the URL and restored next time."
      size="lg"
      footer={
        <>
          <Button variant="ghost" icon={<FilterX className="size-3.5" />} onClick={onClearAll} disabled={activeCount === 0}>
            Clear all
          </Button>
          <Button variant="primary" onClick={() => onOpenChange(false)}>
            Show results
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-7">
        <section aria-label="Release year">
          <div className="mb-2 flex items-baseline justify-between">
            <h3 className="font-mono text-[10px] uppercase tracking-[0.12em] text-ink-3">Release year</h3>
            <span className="text-[11.5px] text-ink-3">Inclusive</span>
          </div>
          <div className="grid grid-cols-2 gap-3">
            <label className="flex flex-col gap-1.5 text-[12.5px] text-ink-2">
              From
              <Input
                type="number"
                inputMode="numeric"
                min={1950}
                max={2100}
                value={yearFrom}
                placeholder="1950"
                onChange={(event) => commitNumber("yearFrom", event.target.value, setYearFrom)}
                className="h-9 text-[13px]"
              />
            </label>
            <label className="flex flex-col gap-1.5 text-[12.5px] text-ink-2">
              To
              <Input
                type="number"
                inputMode="numeric"
                min={1950}
                max={2100}
                value={yearTo}
                placeholder="2026"
                onChange={(event) => commitNumber("yearTo", event.target.value, setYearTo)}
                className="h-9 text-[13px]"
              />
            </label>
          </div>
        </section>

        <section aria-label="Playtime">
          <div className="mb-2 flex items-baseline justify-between">
            <h3 className="font-mono text-[10px] uppercase tracking-[0.12em] text-ink-3">Time played</h3>
            <span className="text-[11.5px] text-ink-3">Minutes, for this account</span>
          </div>
          <div className="grid grid-cols-2 gap-3">
            <label className="flex flex-col gap-1.5 text-[12.5px] text-ink-2">
              At least
              <Input
                type="number"
                inputMode="numeric"
                min={0}
                value={playtimeMin}
                placeholder="0"
                onChange={(event) => commitNumber("playtimeMin", event.target.value, setPlaytimeMin)}
                className="h-9 text-[13px]"
              />
            </label>
            <label className="flex flex-col gap-1.5 text-[12.5px] text-ink-2">
              At most
              <Input
                type="number"
                inputMode="numeric"
                min={0}
                value={playtimeMax}
                placeholder="Any"
                onChange={(event) => commitNumber("playtimeMax", event.target.value, setPlaytimeMax)}
                className="h-9 text-[13px]"
              />
            </label>
          </div>
        </section>

        <div className="grid gap-4 sm:grid-cols-2">
          <div className="flex flex-col gap-1.5">
            <span className="text-[12.5px] text-ink-2">Players</span>
            <Select
              label="Minimum player count"
              value={filters.players || "any"}
              onChange={(value) => onChange("players", value)}
              options={PLAYER_OPTIONS}
              className="h-9 text-[12.5px]"
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <span className="text-[12.5px] text-ink-2">Rating</span>
            <Select
              label="Minimum rating"
              value={filters.minRating || "any"}
              onChange={(value) => onChange("minRating", value)}
              options={RATING_OPTIONS}
              className="h-9 text-[12.5px]"
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <span className="text-[12.5px] text-ink-2">Progress</span>
            <Select
              label="Filter by progress state"
              value={filters.state || "any"}
              onChange={(value) => onChange("state", value)}
              options={STATE_OPTIONS}
              className="h-9 text-[12.5px]"
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <span className="text-[12.5px] text-ink-2">Metadata source</span>
            <Select
              label="Filter by metadata provider"
              value={filters.provider || "any"}
              onChange={(value) => onChange("provider", value)}
              options={PROVIDER_OPTIONS}
              className="h-9 text-[12.5px]"
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <span className="text-[12.5px] text-ink-2">Sort by</span>
            <Select
              label="Sort games"
              value={filters.sort}
              onChange={(value) => onChange("sort", value)}
              options={SORT_OPTIONS}
              className="h-9 text-[12.5px]"
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <span className="text-[12.5px] text-ink-2">Direction</span>
            <Segmented
              label="Sort direction"
              value={filters.order}
              onChange={(value) => onChange("order", value)}
              options={[
                { value: "asc", label: "Ascending" },
                { value: "desc", label: "Descending" },
              ]}
              size="sm"
            />
          </div>
        </div>

        <section className="flex flex-wrap items-center gap-x-6 gap-y-3 border-t border-line pt-4" aria-label="Flags">
          <label className="flex cursor-pointer items-center gap-2 text-[12.5px] text-ink-2">
            <Checkbox
              checked={filters.earlyAccess}
              onCheckedChange={(next) => onChange("earlyAccess", next)}
              label="Early access only"
            />
            Early access
          </label>
          {installsAvailable() && (
            <label className="flex cursor-pointer items-center gap-2 text-[12.5px] text-ink-2">
              <Checkbox
                checked={filters.installed}
                onCheckedChange={(next) => onChange("installed", next)}
                label="Installed on this machine"
              />
              Installed on this machine
            </label>
          )}
          {activeCount > 0 && (
            <Badge tone="signal" className="ml-auto">
              {activeCount} active
            </Badge>
          )}
        </section>
      </div>
    </Dialog>
  );
}
