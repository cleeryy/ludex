import { useEffect, useMemo, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { CircleAlert, ExternalLink, Search } from "lucide-react";

import { api, assetUrl } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import {
  candidateKey,
  confidencePercent,
  filterByYear,
  preservedFields,
  previewChanges,
  type MatchField,
} from "@/lib/metadata-match";
import type { GameDetail, MetadataCandidate, MetadataProviderStatus } from "@/lib/types";
import { Badge, Button, Checkbox, Field, Input, Skeleton } from "@/components/ui/controls";
import { Dialog } from "@/components/ui/dialog";
import { useToast } from "@/state/toasts";
import { t } from "@/lib/i18n";

/**
 * Editor-only dialog for the manual metadata match (#135).
 *
 * The search runs once the dialog opens (prefilled with the game title), never
 * on a normal page load. Picking a candidate previews the changes; only the
 * Apply button writes anything, and hand-edited fields are kept unless the
 * editor explicitly replaces them.
 */
export function MatchMetadataDialog({
  open,
  onOpenChange,
  game,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  game: GameDetail;
}) {
  const queryClient = useQueryClient();
  const { push } = useToast();
  const [query, setQuery] = useState(game.title);
  // Empty by default: the stored year often belongs to the wrong match, so the
  // filter is opt-in. The placeholder shows the current one.
  const [year, setYear] = useState("");
  const [provider, setProvider] = useState<string | null>(null);
  const [submitted, setSubmitted] = useState({ q: game.title, year: "", provider: null as string | null });
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const [replaceManual, setReplaceManual] = useState(false);
  const [applying, setApplying] = useState(false);
  const [applyError, setApplyError] = useState<string | null>(null);
  // Chips come from the unfiltered search, so filtering to one provider does
  // not remove the "All providers" way back.
  const [providerList, setProviderList] = useState<MetadataProviderStatus[]>([]);
  const optionRefs = useRef(new Map<string, HTMLLIElement>());

  // A new game (or reopening the dialog) starts from its current title.
  useEffect(() => {
    if (!open) return;
    setQuery(game.title);
    setYear("");
    setProvider(null);
    setSubmitted({ q: game.title, year: "", provider: null });
    setSelectedKey(null);
    setReplaceManual(false);
    setApplyError(null);
    setProviderList([]);
  }, [open, game.id, game.title, game.releaseDate]);

  const search = useQuery({
    queryKey: ["metadata-candidates", game.id, submitted.q, submitted.year, submitted.provider ?? "*"],
    queryFn: () =>
      api.metadataCandidates(game.id, {
        q: submitted.q,
        provider: submitted.provider ?? undefined,
        year: submitted.year ? Number(submitted.year) : undefined,
      }),
    enabled: open && submitted.q.trim().length > 0,
    staleTime: 30_000,
    retry: false,
  });

  useEffect(() => {
    if (search.data && submitted.provider === null) setProviderList(search.data.providers);
  }, [search.data, submitted.provider]);

  const manualFields = search.data?.manualFields ?? [];
  const selectedYear = submitted.year ? Number(submitted.year) : null;
  const candidates = useMemo(
    () => filterByYear(search.data?.candidates ?? [], selectedYear),
    [search.data, selectedYear],
  );
  const activeKey = useMemo(() => {
    if (selectedKey && candidates.some((candidate) => candidateKey(candidate) === selectedKey)) {
      return selectedKey;
    }
    const current = candidates.find((candidate) => candidate.selected);
    return current ? candidateKey(current) : candidates[0] ? candidateKey(candidates[0]) : null;
  }, [candidates, selectedKey]);
  const selected = candidates.find((candidate) => candidateKey(candidate) === activeKey) ?? null;
  const preview = selected ? previewChanges(game, selected, manualFields) : [];
  const preserved = preservedFields(manualFields, replaceManual);
  const providerErrors = (search.data?.providers ?? []).filter((status) => !status.ok);

  const submit = (nextProvider: string | null = provider) => {
    const trimmed = query.trim();
    if (!trimmed) return;
    setProvider(nextProvider);
    setSelectedKey(null);
    setSubmitted({ q: trimmed, year, provider: nextProvider });
  };

  const focus = (candidate: MetadataCandidate) => {
    setSelectedKey(candidateKey(candidate));
    optionRefs.current.get(candidateKey(candidate))?.focus();
  };

  const move = (delta: 1 | -1) => {
    if (!candidates.length) return;
    const index = candidates.findIndex((candidate) => candidateKey(candidate) === activeKey);
    const next = candidates[(index + delta + candidates.length) % candidates.length];
    if (next) focus(next);
  };

  const onListKeyDown = (event: React.KeyboardEvent<HTMLUListElement>) => {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      move(1);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      move(-1);
    } else if (event.key === "Home" && candidates[0]) {
      event.preventDefault();
      focus(candidates[0]);
    } else if (event.key === "End") {
      const last = candidates[candidates.length - 1];
      if (last) {
        event.preventDefault();
        focus(last);
      }
    }
  };

  const apply = async () => {
    if (!selected) return;
    setApplying(true);
    setApplyError(null);
    try {
      await api.applyMetadata(game.id, {
        provider: selected.provider,
        providerId: selected.providerId,
        replaceManual,
      });
      await queryClient.invalidateQueries({ queryKey: ["game", game.id] });
      await queryClient.invalidateQueries({ queryKey: ["games"] });
      push({
        title: t("Metadata matched"),
        description: t("The title, cover and provider record now follow the selected match."),
        tone: "success",
      });
      onOpenChange(false);
    } catch (cause) {
      setApplyError(errorMessage(cause));
    } finally {
      setApplying(false);
    }
  };

  const status = search.isPending
    ? t("Searching providers…")
    : search.isError
      ? t("The provider search failed.")
      : t("{count} candidates", { count: candidates.length });

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title={t("Match metadata")}
      description={t("Search the configured providers and pick the correct record. Nothing changes until you apply it.")}
      size="lg"
      footer={
        <>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            {t("Cancel")}
          </Button>
          <Button
            variant="primary"
            icon={<Search className="size-4" />}
            loading={applying}
            disabled={!selected}
            onClick={() => void apply()}
          >
            {t("Apply match")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <form
          className="grid grid-cols-[minmax(0,1fr)_110px_auto] items-end gap-3"
          onSubmit={(event) => {
            event.preventDefault();
            submit();
          }}
        >
          <Field label={t("Search")} htmlFor="match-metadata-query">
            <Input
              id="match-metadata-query"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder={game.title}
              autoComplete="off"
              autoFocus
            />
          </Field>
          <Field label={t("Release year")} htmlFor="match-metadata-year">
            <Input
              id="match-metadata-year"
              type="number"
              inputMode="numeric"
              min={1900}
              max={2200}
              value={year}
              onChange={(event) => setYear(event.target.value.replace(/[^0-9]/g, "").slice(0, 4))}
              placeholder={game.releaseDate ? game.releaseDate.slice(0, 4) : "2025"}
            />
          </Field>
          <Button type="submit" icon={<Search className="size-4" />} disabled={!query.trim()} loading={search.isFetching}>
            {t("Search")}
          </Button>
        </form>

        {providerList.length > 1 && (
          <div className="flex flex-wrap items-center gap-1.5" role="group" aria-label={t("Providers")}>
            <ProviderChip
              active={provider === null}
              label={t("All providers")}
              onClick={() => submit(null)}
            />
            {providerList.map((status) => (
              <ProviderChip
                key={status.slug}
                active={provider === status.slug}
                label={
                  status.ok
                    ? t("{name} ({count})", { name: status.name, count: status.candidates })
                    : t("{name} (unavailable)", { name: status.name })
                }
                tone={status.ok ? "default" : "danger"}
                onClick={() => submit(status.slug)}
              />
            ))}
          </div>
        )}

        <p className="text-[12px] text-ink-3" aria-live="polite">
          {status}
          {selectedYear !== null && search.data && (
            <> · {t("Year filter: {year}", { year: selectedYear })}</>
          )}
        </p>

        {providerErrors.length > 0 && (
          <div className="border-l-2 border-danger pl-3" role="status">
            <p className="flex items-center gap-1.5 text-[12.5px] font-medium text-danger">
              <CircleAlert className="size-3.5" aria-hidden="true" />
              {t("Some providers did not answer")}
            </p>
            <ul className="mt-1 flex flex-col gap-0.5 text-[12px] text-ink-2">
              {providerErrors.map((status) => (
                <li key={status.slug}>
                  <span className="font-medium">{status.name}</span>
                  {status.error ? ` — ${status.error}` : ""}
                </li>
              ))}
            </ul>
          </div>
        )}

        {search.isError && (
          <div className="flex flex-col gap-2 border border-line p-3">
            <p className="text-[12.5px] text-danger">{errorMessage(search.error)}</p>
            <div>
              <Button size="sm" onClick={() => void search.refetch()}>
                {t("Try again")}
              </Button>
            </div>
          </div>
        )}

        {search.isPending && (
          <div className="flex flex-col gap-2">
            <Skeleton className="h-16 w-full" />
            <Skeleton className="h-16 w-full" />
          </div>
        )}

        {!search.isPending && !search.isError && candidates.length === 0 && (
          <p className="border border-line p-4 text-center text-[13px] text-ink-2">
            {t("No candidate matches this search. Try a shorter title or another year.")}
          </p>
        )}

        {candidates.length > 0 && (
          <ul
            className="flex max-h-[320px] flex-col gap-1.5 overflow-y-auto"
            role="listbox"
            aria-label={t("Metadata candidates")}
            aria-activedescendant={activeKey ? `candidate-${activeKey}` : undefined}
            onKeyDown={onListKeyDown}
          >
            {candidates.map((candidate) => {
              const key = candidateKey(candidate);
              const active = key === activeKey;
              return (
                <li
                  key={key}
                  id={`candidate-${key}`}
                  ref={(element) => {
                    if (element) optionRefs.current.set(key, element);
                    else optionRefs.current.delete(key);
                  }}
                  role="option"
                  aria-selected={active}
                  tabIndex={active ? 0 : -1}
                  onClick={() => setSelectedKey(key)}
                  onKeyDown={(event) => {
                    if (event.key === "Enter" || event.key === " ") {
                      event.preventDefault();
                      setSelectedKey(key);
                    }
                  }}
                  className={
                    active
                      ? "flex cursor-pointer gap-3 rounded-[2px] border border-signal/60 bg-surface-2 p-2.5 outline-none ring-1 ring-signal/30"
                      : "flex cursor-pointer gap-3 rounded-[2px] border border-line bg-surface p-2.5 hover:border-line-2"
                  }
                >
                  <div className="flex h-[64px] w-[48px] shrink-0 items-center justify-center overflow-hidden rounded-[2px] border border-line bg-bg-2">
                    {assetUrl(candidate.coverUrl) ? (
                      <img
                        src={assetUrl(candidate.coverUrl) ?? undefined}
                        alt=""
                        loading="lazy"
                        className="size-full object-cover"
                      />
                    ) : (
                      <span className="text-[10px] text-ink-3">—</span>
                    )}
                  </div>
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-1.5">
                      <span className="text-[13px] font-medium text-ink">{candidate.title}</span>
                      {candidate.year !== null && (
                        <span className="font-mono text-[11px] text-ink-3">{candidate.year}</span>
                      )}
                      <Badge tone="outline">{candidate.providerName}</Badge>
                      {candidate.selected && <Badge tone="ok">{t("Current match")}</Badge>}
                    </div>
                    <p className="mt-0.5 truncate text-[12px] text-ink-2">
                      {candidate.developers.length > 0
                        ? candidate.developers.join(", ")
                        : t("Developer unknown")}
                      {candidate.platforms.length > 0 && (
                        <span className="text-ink-3"> · {candidate.platforms.join(", ")}</span>
                      )}
                    </p>
                    <p className="mt-1 line-clamp-2 text-[11.5px] leading-relaxed text-ink-3">
                      {candidate.summary ?? t("No description.")}
                    </p>
                  </div>
                  <div className="flex shrink-0 flex-col items-end gap-1">
                    <span className="font-mono text-[11px] text-ink-3">
                      {t("{percent}% match", { percent: confidencePercent(candidate) })}
                    </span>
                    {candidate.providerUrl && (
                      <a
                        href={candidate.providerUrl}
                        target="_blank"
                        rel="noreferrer"
                        onClick={(event) => event.stopPropagation()}
                        className="text-ink-3 hover:text-ink"
                        aria-label={t("Open the provider page for {title}", { title: candidate.title })}
                      >
                        <ExternalLink className="size-3.5" />
                      </a>
                    )}
                  </div>
                </li>
              );
            })}
          </ul>
        )}

        {selected && (
          <section className="flex flex-col gap-2 border-t border-line pt-3" aria-live="polite">
            <div className="flex flex-wrap items-baseline justify-between gap-2">
              <h3 className="text-[13px] font-medium text-ink">{t("Changes on apply")}</h3>
              <span className="font-mono text-[11px] text-ink-3">
                {selected.provider} · {selected.providerId}
              </span>
            </div>
            {preview.length === 0 ? (
              <p className="text-[12.5px] text-ink-2">
                {t("This record already matches the game. Applying refreshes the provider id and artwork.")}
              </p>
            ) : (
              <ul className="flex flex-col divide-y divide-line border border-line">
                {preview.map((row) => (
                  <li key={row.field} className="grid grid-cols-[110px_minmax(0,1fr)] gap-3 p-2.5">
                    <span className="text-[12px] font-medium text-ink-2">{matchLabel(row.field)}</span>
                    <div className="min-w-0 text-[12.5px]">
                      <p className="truncate text-ink-3 line-through">{row.current || t("Empty")}</p>
                      <p className="truncate text-ink">{row.next}</p>
                      {row.manual && !replaceManual && (
                        <Badge tone="signal" className="mt-1">
                          {t("Kept: edited by hand")}
                        </Badge>
                      )}
                    </div>
                  </li>
                ))}
              </ul>
            )}
            <p className="text-[12px] leading-relaxed text-ink-3">
              {t("Genres, publishers, rating, age rating, player count and alternative titles also follow the selected record. Its cover, background, screenshots and trailers are cached on apply.")}
            </p>
            {manualFields.length > 0 && (
              <div className="flex items-start gap-2.5">
                <Checkbox
                  checked={replaceManual}
                  onCheckedChange={setReplaceManual}
                  label={t("Replace fields I edited by hand")}
                />
                <div className="text-[12.5px]">
                  <p className="text-ink">{t("Replace fields I edited by hand")}</p>
                  <p className="text-ink-3">
                    {replaceManual
                      ? t("Every field is taken from the selected record.")
                      : preserved.length > 0
                        ? t("Kept: {fields}", {
                            fields: preserved.map(matchLabel).join(", "),
                          })
                        : t("Nothing was edited by hand.")}
                  </p>
                </div>
              </div>
            )}
            {applyError && (
              <p className="border-l-2 border-danger pl-3 text-[12.5px] text-danger" role="alert">
                {applyError}
              </p>
            )}
          </section>
        )}
      </div>
    </Dialog>
  );
}

function ProviderChip({
  active,
  label,
  tone = "default",
  onClick,
}: {
  active: boolean;
  label: string;
  tone?: "default" | "danger";
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      aria-pressed={active}
      onClick={onClick}
      className={
        active
          ? "inline-flex h-6 items-center rounded-[2px] border border-signal/60 bg-signal/12 px-2 text-[11.5px] font-medium text-signal-text"
          : tone === "danger"
            ? "inline-flex h-6 items-center rounded-[2px] border border-danger/40 px-2 text-[11.5px] text-danger hover:border-danger/70"
            : "inline-flex h-6 items-center rounded-[2px] border border-line-2 px-2 text-[11.5px] text-ink-2 hover:border-ink-3"
      }
    >
      {label}
    </button>
  );
}

/** Translated field label; literal `t()` calls so the catalog picks them up. */
function matchLabel(field: MatchField): string {
  switch (field) {
    case "title":
      return t("Title");
    case "releaseDate":
      return t("Release year");
    case "description":
      return t("Description");
    case "genres":
      return t("Genres");
    case "developers":
      return t("Developers");
    case "publishers":
      return t("Publishers");
    case "rating":
      return t("Rating");
    case "ageRating":
      return t("Age rating");
    case "altTitles":
      return t("Alternative titles");
    case "players":
      return t("Players");
  }
}
