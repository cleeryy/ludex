import { useEffect, useId, useRef, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { Search } from "lucide-react";
import { cn } from "@/lib/cn";
import { t } from "@/lib/i18n";
import {
  filterSettingsSections,
  settingsSectionPath,
  type SettingsSectionId,
  type SettingsSectionMeta,
} from "@/lib/settings-sections";
import { SettingsIcon } from "@/components/settings/settings-icons";
import { settingsSectionLabel } from "@/components/settings/settings-copy";

/**
 * In-page category nav (#174): a sticky column on desktop, a sticky chip row
 * on phones. Entries are links, so a category can be opened in a new tab, and
 * the current one carries `aria-current="location"` — this is a place inside a
 * page, not another page.
 *
 * The search field filters the categories by their translated label and their
 * keywords; Enter opens the first match. Nothing here traps focus.
 */
export function SettingsNav({
  sections,
  active,
  onSelect,
}: {
  sections: SettingsSectionMeta[];
  active: SettingsSectionId;
  onSelect: () => void;
}) {
  const navigate = useNavigate();
  const [query, setQuery] = useState("");
  const desktopSearchId = useId();
  const mobileSearchId = useId();
  const mobileListRef = useRef<HTMLUListElement>(null);
  const filtered = filterSettingsSections(query, sections, (section) => settingsSectionLabel(section.id));

  // A deep link can land on a chip that is off-screen; bring it into view
  // without moving the page vertically.
  useEffect(() => {
    const activeChip = mobileListRef.current?.querySelector<HTMLElement>('[aria-current="location"]');
    activeChip?.scrollIntoView({ block: "nearest", inline: "center" });
  }, [active]);

  const openFirstMatch = (event: React.KeyboardEvent<HTMLInputElement>) => {
    if (event.key !== "Enter") return;
    const first = filtered[0];
    if (!first) return;
    event.preventDefault();
    onSelect();
    navigate(settingsSectionPath(first.id));
  };

  const searchField = (id: string, className: string) => (
    <div className={cn("relative", className)}>
      <Search className="pointer-events-none absolute start-3 top-1/2 size-3.5 -translate-y-1/2 text-ink-3" aria-hidden="true" />
      <label htmlFor={id} className="sr-only">
        {t("Find a setting")}
      </label>
      <input
        id={id}
        type="search"
        value={query}
        onChange={(event) => setQuery(event.target.value)}
        onKeyDown={openFirstMatch}
        placeholder={t("Find a setting")}
        autoComplete="off"
        spellCheck={false}
        className="h-10 w-full rounded-[2px] border border-line bg-surface ps-9 pe-3 text-[13px] text-ink transition-colors placeholder:text-ink-3 focus:border-line-2"
      />
    </div>
  );

  return (
    <>
      <nav aria-label={t("Settings sections")} className="hidden lg:sticky lg:top-0 lg:block lg:w-[228px] lg:shrink-0">
        {searchField(desktopSearchId, "")}
        <p className="pb-2 pt-5 text-[10.5px] uppercase tracking-[0.12em] text-ink-3">{t("Categories")}</p>
        <ul className="border-t border-line">
          {filtered.map((section) => {
            const current = section.id === active;
            return (
              <li key={section.id}>
                <Link
                  to={settingsSectionPath(section.id)}
                  onClick={onSelect}
                  aria-current={current ? "location" : undefined}
                  data-active={current}
                  className="rail-link h-11 w-full pe-3"
                >
                  <span className="grid size-4 place-items-center opacity-80">
                    <SettingsIcon name={section.icon} className="size-4" />
                  </span>
                  <span className="truncate">{settingsSectionLabel(section.id)}</span>
                </Link>
              </li>
            );
          })}
        </ul>
        {filtered.length === 0 && (
          <p className="px-1 py-3 text-[12px] leading-relaxed text-ink-3">{t("No section matches “{query}”.", { query })}</p>
        )}
      </nav>

      <nav
        aria-label={t("Settings sections")}
        className="sticky top-0 z-20 -mx-[var(--page-x)] border-b border-line bg-bg px-[var(--page-x)] py-2 lg:hidden"
      >
        {searchField(mobileSearchId, "")}
        <ul ref={mobileListRef} className="nav-scroll -mx-1 mt-1.5 flex gap-0.5 overflow-x-auto">
          {filtered.map((section) => {
            const current = section.id === active;
            return (
              <li key={section.id} className="shrink-0">
                <Link
                  to={settingsSectionPath(section.id)}
                  onClick={onSelect}
                  aria-current={current ? "location" : undefined}
                  className={cn(
                    "flex h-11 items-center gap-1.5 border-b-2 px-2.5 text-[12.5px] transition-colors",
                    current ? "border-signal text-ink" : "border-transparent text-ink-3 hover:text-ink-2",
                  )}
                >
                  <SettingsIcon name={section.icon} className="size-3.5" />
                  {settingsSectionLabel(section.id)}
                </Link>
              </li>
            );
          })}
        </ul>
        {filtered.length === 0 && (
          <p className="px-3 py-2 text-[12px] leading-relaxed text-ink-3">{t("No section matches “{query}”.", { query })}</p>
        )}
      </nav>
    </>
  );
}
