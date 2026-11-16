import { useEffect, useRef } from "react";
import { Link } from "react-router-dom";
import {
  CloudUpload,
  FolderArchive,
  Gamepad2,
  Info,
  Rocket,
  SlidersHorizontal,
  Trophy,
  type LucideIcon,
} from "lucide-react";
import { cn } from "@/lib/cn";
import { t } from "@/lib/i18n";
import { centeredScrollLeft } from "@/lib/scroll";
import {
  gameSectionLabel,
  gameSectionPath,
  type GameSectionId,
  type GameSectionMeta,
} from "@/lib/game-sections";

const ICONS: Record<GameSectionId, LucideIcon> = {
  overview: Info,
  rom: Gamepad2,
  files: FolderArchive,
  launch: Rocket,
  settings: SlidersHorizontal,
  saves: CloudUpload,
  achievements: Trophy,
};

/**
 * In-page group nav for the game page (#189), the same two-surface shape as
 * the settings categories: a sticky rail on desktop, a sticky chip row on
 * phones. Entries are links so a group can be opened in a new tab, and the
 * current one carries `aria-current="location"` — this is a place inside a
 * page, not another page.
 */
export function GameNav({
  gameId,
  sections,
  active,
  onSelect,
}: {
  gameId: number;
  sections: readonly GameSectionMeta[];
  active: GameSectionId;
  /** Bumps the focus token so the new group's heading takes focus. */
  onSelect: () => void;
}) {
  const listRef = useRef<HTMLUListElement>(null);

  // A deep link can land on a chip that is off-screen. Center it on the
  // horizontal axis only (#237): `scrollIntoView` also scrolled the vertical
  // ancestor, which opened `/game/:id` 796 px down on a phone.
  useEffect(() => {
    const list = listRef.current;
    const activeChip = list?.querySelector<HTMLElement>('[aria-current="location"]');
    if (!list || !activeChip) return;
    const listBox = list.getBoundingClientRect();
    const chipBox = activeChip.getBoundingClientRect();
    list.scrollLeft = centeredScrollLeft({
      viewportWidth: list.clientWidth,
      contentWidth: list.scrollWidth,
      viewportLeft: listBox.left,
      targetLeft: chipBox.left,
      targetWidth: chipBox.width,
      scrollLeft: list.scrollLeft,
    });
  }, [active]);

  return (
    <>
      <nav
        aria-label={t("Game sections")}
        className="hidden lg:sticky lg:top-0 lg:block lg:w-[228px] lg:shrink-0"
      >
        <p className="pb-2 text-meta uppercase tracking-[0.12em] text-ink-3">{t("Sections")}</p>
        <ul className="border-t border-line">
          {sections.map((section) => {
            const current = section.id === active;
            const Icon = ICONS[section.id];
            return (
              <li key={section.id}>
                <Link
                  to={gameSectionPath(gameId, section.id)}
                  onClick={onSelect}
                  aria-current={current ? "location" : undefined}
                  data-active={current}
                  className="rail-link h-11 w-full pe-3"
                >
                  <span className="grid size-4 place-items-center opacity-80">
                    <Icon className="size-4" aria-hidden="true" />
                  </span>
                  <span className="truncate">{gameSectionLabel(section.id)}</span>
                </Link>
              </li>
            );
          })}
        </ul>
      </nav>

      <nav
        aria-label={t("Game sections")}
        className="sticky top-0 z-20 -mx-[var(--page-x)] border-b border-line bg-bg px-[var(--page-x)] py-2 lg:hidden"
      >
        <ul ref={listRef} className="nav-scroll -mx-1 flex gap-0.5 overflow-x-auto">
          {sections.map((section) => {
            const current = section.id === active;
            const Icon = ICONS[section.id];
            return (
              <li key={section.id} className="shrink-0">
                <Link
                  to={gameSectionPath(gameId, section.id)}
                  onClick={onSelect}
                  aria-current={current ? "location" : undefined}
                  className={cn(
                    "flex h-11 items-center gap-1.5 border-b-2 px-2.5 text-label transition-colors",
                    current ? "border-signal text-ink" : "border-transparent text-ink-3 hover:text-ink-2",
                  )}
                >
                  <Icon className="size-3.5" aria-hidden="true" />
                  {gameSectionLabel(section.id)}
                </Link>
              </li>
            );
          })}
        </ul>
      </nav>
    </>
  );
}
