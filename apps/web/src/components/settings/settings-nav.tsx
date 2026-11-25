import { useEffect, useId, useRef, useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { Search } from "lucide-react";
import { cn } from "@/lib/cn";
import { t } from "@/lib/i18n";
import {
  filterSettingsSections,
  settingsSectionPath,
  visibleSettingsSections,
  type SettingsSectionId,
  type SettingsSectionMeta,
} from "@/lib/settings-sections";
import { setAdvancedTiersUnlocked, useAdvancedTiersUnlocked } from "@/lib/settings-tiers";
import { Badge, Switch } from "@/components/ui/controls";
import { SettingsIcon } from "@/components/settings/settings-icons";
import { settingsSectionLabel } from "@/components/settings/settings-copy";

/**
 * In-page category nav (#174, #347): a sticky column on desktop, a sticky chip
 * row on phones. Everyday categories show by default; advanced categories
 * appear under Power-user mode (or when searched or deep-linked).
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
  const unlocked = useAdvancedTiersUnlocked();
  const desktopSearchId = useId();
  const mobileSearchId = useId();
  const mobileListRef = useRef<HTMLUListElement>(null);
  const searching = query.trim().length > 0;
  const visible = visibleSettingsSections(sections, unlocked, active);
  const filtered = searching
    ? filterSettingsSections(query, sections, (section) => settingsSectionLabel(section.id))
    : visible;
  const defaultSections = filtered.filter((section) => section.tier === "default");
  const advancedSections = filtered.filter((section) => section.tier === "advanced");

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
        className="h-10 w-full rounded-[var(--r-sm)] border border-line bg-surface ps-9 pe-3 text-body text-ink transition-colors placeholder:text-ink-3 focus:border-line-2"
      />
    </div>
  );

  const renderDesktopItem = (section: SettingsSectionMeta, showAdvancedBadge: boolean) => {
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
          {showAdvancedBadge && (
            <Badge tone="outline" className="ms-auto shrink-0">
              {t("Advanced")}
            </Badge>
          )}
        </Link>
      </li>
    );
  };

  return (
    <>
      <nav aria-label={t("Settings sections")} className="hidden lg:sticky lg:top-0 lg:block lg:w-[228px] lg:shrink-0">
        {searchField(desktopSearchId, "")}
        {searching ? (
          <>
            <p className="pb-2 pt-5 text-micro uppercase tracking-[0.12em] text-ink-3">{t("Categories")}</p>
            <ul className="border-t border-line">
              {filtered.map((section) => renderDesktopItem(section, !unlocked && section.tier === "advanced"))}
            </ul>
          </>
        ) : (
          <>
            <p className="pb-2 pt-5 text-micro uppercase tracking-[0.12em] text-ink-3">{t("Categories")}</p>
            <ul className="border-t border-line">
              {defaultSections.map((section) => renderDesktopItem(section, false))}
            </ul>
            {advancedSections.length > 0 && (
              <>
                <p className="pb-2 pt-5 text-micro uppercase tracking-[0.12em] text-ink-3">{t("Advanced")}</p>
                <ul className="border-t border-line">
                  {advancedSections.map((section) => renderDesktopItem(section, false))}
                </ul>
              </>
            )}
          </>
        )}
        {filtered.length === 0 && (
          <p className="px-1 py-3 text-label leading-relaxed text-ink-3">{t("No section matches “{query}”.", { query })}</p>
        )}
        <div className="mt-6 border-t border-line pt-4">
          <div className="flex items-start justify-between gap-3">
            <div className="min-w-0">
              <p className="text-label font-medium text-ink">{t("Power-user mode")}</p>
              <p className="mt-1 text-micro leading-relaxed text-ink-3">
                {t("Show advanced machine, streaming, compatibility, and launch options on this device.")}
              </p>
            </div>
            <Switch
              checked={unlocked}
              label={t("Power-user mode")}
              onCheckedChange={(next) => setAdvancedTiersUnlocked(next)}
            />
          </div>
        </div>
      </nav>

      <nav
        aria-label={t("Settings sections")}
        className="sticky top-0 z-20 -mx-[var(--page-x)] border-b border-line bg-bg px-[var(--page-x)] py-2 lg:hidden"
      >
        <div className="flex items-center gap-3">
          {searchField(mobileSearchId, "flex-1")}
          <div className="flex shrink-0 items-center gap-2">
            <span className="text-micro text-ink-3">{t("Advanced")}</span>
            <Switch
              checked={unlocked}
              label={t("Power-user mode")}
              onCheckedChange={(next) => setAdvancedTiersUnlocked(next)}
            />
          </div>
        </div>
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
                    "flex h-11 items-center gap-1.5 border-b-2 px-2.5 text-label transition-colors",
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
          <p className="px-3 py-2 text-label leading-relaxed text-ink-3">{t("No section matches “{query}”.", { query })}</p>
        )}
      </nav>
    </>
  );
}
