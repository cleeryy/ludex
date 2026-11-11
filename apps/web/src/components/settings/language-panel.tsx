import { cn } from "@/lib/cn";
import {
  coverageFor,
  isSelectable,
  LOCALES,
  statusLabel,
  t,
  useI18n,
  type LocaleMeta,
} from "@/lib/i18n";
import { SectionHeader } from "@/components/page";
import { Badge } from "@/components/ui/controls";

/**
 * Language picker (Settings → Language). Every locale the coverage gate
 * allows is listed with its native name, English name, direction, coverage and
 * review state. Changing the language is instant: `setLocale` loads the catalog
 * and remounts the tree without a reload.
 *
 * Locales below the gate (95 % translated, `SELECTABLE_PERCENT`) are not
 * listed: they would mix languages. `?lang=xx` still opens one for reviewers,
 * and the coverage table in `docs/I18N.md` lists what is in flight.
 */
export function LanguagePanel() {
  const { locale: activeLocale, setLocale } = useI18n();
  const official = LOCALES.filter((entry) => entry.status === "official");
  const previews = LOCALES.filter(
    (entry) => entry.status === "preview" && isSelectable(entry.code),
  ).sort((a, b) => (coverageFor(b.code)?.percent ?? 0) - (coverageFor(a.code)?.percent ?? 0));
  const groups = [
    { key: "official", title: t("Official languages"), entries: official },
    { key: "preview", title: t("Community translations"), entries: previews },
  ].filter((group) => group.entries.length > 0);

  return (
    <div>
      <SectionHeader
        title={t("Language")}
        description={t("Applies to this browser or app window. Community translations fall back to English for missing strings.")}
      />
      {groups.map((group) => (
        <div key={group.key} className="mt-4 first:mt-0">
          <p className="mb-2 font-mono text-[10px] uppercase tracking-[0.12em] text-ink-3">{group.title}</p>
          <div className="grid gap-1.5 sm:grid-cols-2">
            {group.entries.map((entry) => (
              <LocaleButton
                key={entry.code}
                entry={entry}
                selected={activeLocale === entry.code}
                onSelect={() => void setLocale(entry.code)}
              />
            ))}
          </div>
        </div>
      ))}
      <p className="mt-3 text-[11.5px] leading-relaxed text-ink-3">
        {t("Community locales are contributed as JSON files. See docs/I18N.md for the contribution flow; Arabic is machine-seeded and unreviewed.")}
      </p>
    </div>
  );
}

function LocaleButton({
  entry,
  selected,
  onSelect,
}: {
  entry: LocaleMeta;
  selected: boolean;
  onSelect: () => void;
}) {
  const coverage = coverageFor(entry.code);
  return (
    <button
      type="button"
      lang={entry.code}
      dir={entry.dir}
      aria-pressed={selected}
      onClick={onSelect}
      className={cn(
        "flex min-h-11 items-center justify-between gap-3 border px-3 py-2 text-start transition-colors",
        selected ? "border-signal bg-surface-2" : "border-line hover:border-line-2 hover:bg-surface",
      )}
    >
      <span className="min-w-0">
        <span className="block truncate text-[13px] text-ink">{entry.label}</span>
        <span className="block truncate font-mono text-[10px] text-ink-3">
          {entry.englishLabel}
          {entry.dir === "rtl" ? " · RTL" : ""}
        </span>
      </span>
      <span className="flex shrink-0 items-center gap-1.5">
        {entry.code !== "en" && coverage && <span className="num text-[10px] text-ink-3">{coverage.percent}%</span>}
        <Badge tone={selected ? "signal" : "outline"}>{statusLabel(entry)}</Badge>
      </span>
    </button>
  );
}
