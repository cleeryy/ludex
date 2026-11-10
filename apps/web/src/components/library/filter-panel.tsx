import { useEffect, useState } from "react";
import { FilterX } from "lucide-react";
import { installsAvailable } from "@/lib/install";
import {
  playerOptions,
  providerOptions,
  ratingOptions,
  sortOptions,
  stateOptions,
  type FilterKey,
  type LibraryFilters,
} from "@/lib/library-filters";
import { Badge, Button, Checkbox, Input, Segmented } from "@/components/ui/controls";
import { Dialog } from "@/components/ui/dialog";
import { Select } from "@/components/ui/select";
import { t } from "@/lib/i18n";

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
      title={t("Filters")}
      description={t("Narrow the shelf. Every filter is saved in the URL and restored next time.")}
      size="lg"
      footer={
        <>
          <Button variant="ghost" icon={<FilterX className="size-3.5" />} onClick={onClearAll} disabled={activeCount === 0}>
            {t("Clear all")}
          </Button>
          <Button variant="primary" onClick={() => onOpenChange(false)}>
            {t("Show results")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-7">
        <section aria-label={t("Release year")}>
          <div className="mb-2 flex items-baseline justify-between">
            <h3 className="font-mono text-[10px] uppercase tracking-[0.12em] text-ink-3">{t("Release year")}</h3>
            <span className="text-[11.5px] text-ink-3">{t("Inclusive")}</span>
          </div>
          <div className="grid grid-cols-2 gap-3">
            <label className="flex flex-col gap-1.5 text-[12.5px] text-ink-2">
              {t("From")}
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
              {t("To")}
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

        <section aria-label={t("Playtime")}>
          <div className="mb-2 flex items-baseline justify-between">
            <h3 className="font-mono text-[10px] uppercase tracking-[0.12em] text-ink-3">{t("Time played")}</h3>
            <span className="text-[11.5px] text-ink-3">{t("Minutes, for this account")}</span>
          </div>
          <div className="grid grid-cols-2 gap-3">
            <label className="flex flex-col gap-1.5 text-[12.5px] text-ink-2">
              {t("At least")}
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
              {t("At most")}
              <Input
                type="number"
                inputMode="numeric"
                min={0}
                value={playtimeMax}
                placeholder={t("Any")}
                onChange={(event) => commitNumber("playtimeMax", event.target.value, setPlaytimeMax)}
                className="h-9 text-[13px]"
              />
            </label>
          </div>
        </section>

        <div className="grid gap-4 sm:grid-cols-2">
          <div className="flex flex-col gap-1.5">
            <span className="text-[12.5px] text-ink-2">{t("Players")}</span>
            <Select
              label={t("Minimum player count")}
              value={filters.players || "any"}
              onChange={(value) => onChange("players", value)}
              options={playerOptions()}
              className="h-9 text-[12.5px]"
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <span className="text-[12.5px] text-ink-2">{t("Rating")}</span>
            <Select
              label={t("Minimum rating")}
              value={filters.minRating || "any"}
              onChange={(value) => onChange("minRating", value)}
              options={ratingOptions()}
              className="h-9 text-[12.5px]"
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <span className="text-[12.5px] text-ink-2">{t("Progress")}</span>
            <Select
              label={t("Filter by progress state")}
              value={filters.state || "any"}
              onChange={(value) => onChange("state", value)}
              options={stateOptions()}
              className="h-9 text-[12.5px]"
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <span className="text-[12.5px] text-ink-2">{t("Metadata source")}</span>
            <Select
              label={t("Filter by metadata provider")}
              value={filters.provider || "any"}
              onChange={(value) => onChange("provider", value)}
              options={providerOptions()}
              className="h-9 text-[12.5px]"
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <span className="text-[12.5px] text-ink-2">{t("Sort by")}</span>
            <Select
              label={t("Sort games")}
              value={filters.sort}
              onChange={(value) => onChange("sort", value)}
              options={sortOptions()}
              className="h-9 text-[12.5px]"
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <span className="text-[12.5px] text-ink-2">{t("Direction")}</span>
            <Segmented
              label={t("Sort direction")}
              value={filters.order}
              onChange={(value) => onChange("order", value)}
              options={[
                { value: "asc", label: t("Ascending") },
                { value: "desc", label: t("Descending") },
              ]}
              size="sm"
            />
          </div>
        </div>

        <section className="flex flex-wrap items-center gap-x-6 gap-y-3 border-t border-line pt-4" aria-label={t("Flags")}>
          <label className="flex cursor-pointer items-center gap-2 text-[12.5px] text-ink-2">
            <Checkbox
              checked={filters.earlyAccess}
              onCheckedChange={(next) => onChange("earlyAccess", next)}
              label={t("Early access only")}
            />
            {t("Early access")}
          </label>
          {installsAvailable() && (
            <label className="flex cursor-pointer items-center gap-2 text-[12.5px] text-ink-2">
              <Checkbox
                checked={filters.installed}
                onCheckedChange={(next) => onChange("installed", next)}
                label={t("Installed on this machine")}
              />
              {t("Installed on this machine")}
            </label>
          )}
          {activeCount > 0 && (
            <Badge tone="signal" className="ml-auto">
              {t("{count} active", { count: activeCount })}
            </Badge>
          )}
        </section>
      </div>
    </Dialog>
  );
}
