import { useEffect, useId, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useNavigate } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import {
  ArrowRight,
  Dice5,
  Gamepad2,
  Keyboard,
  ListOrdered,
  Moon,
  Plus,
  RefreshCw,
  Search,
  Settings,
  SlidersHorizontal,
  Sun,
  UserRound,
} from "lucide-react";
import { api } from "@/lib/api";
import { installsAvailable } from "@/lib/install";
import { isNative } from "@/lib/native";
import { cn } from "@/lib/cn";
import { formatYear, stateLabel } from "@/lib/format";
import { scoreSearch } from "@/lib/search";
import { availableSettingsSections, settingsSectionPath } from "@/lib/settings-sections";
import { navGroups, systemNav, type NavContext, type NavDestination } from "@/lib/navigation";
import { openFiltersPanel, openShortcutsHelp } from "@/lib/ui-events";
import { useAuth, useIsAdmin, useIsEditor } from "@/state/auth";
import { otherModeTheme, useSettings } from "@/state/settings";
import { useToast } from "@/state/toasts";
import { GameCover } from "@/components/art/cover";
import { Kbd } from "@/components/ui/controls";
import { NavIcon } from "@/components/shell/nav-icons";
import { navGroupLabel, navLabel } from "@/components/shell/nav-copy";
import { settingsSectionLabel } from "@/components/settings/settings-copy";
import { SettingsIcon } from "@/components/settings/settings-icons";
import { t } from "@/lib/i18n";

interface Item {
  id: string;
  label: string;
  hint?: string;
  icon: React.ReactNode;
  kbd?: string;
  /** Extra words the fuzzy search may match, beyond the label. */
  keywords?: string[];
  run: () => void;
}

interface Section {
  id: string;
  label: string;
  items: Item[];
}

export function CommandPalette({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const navigate = useNavigate();
  const { signOut } = useAuth();
  const isEditor = useIsEditor();
  const isAdmin = useIsAdmin();
  const { resolved, setTheme } = useSettings();
  const { push } = useToast();
  const reduce = useReducedMotion();
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const listId = useId();
  const groupIdPrefix = useId();
  const desktop = installsAvailable();

  const { data } = useQuery({
    queryKey: ["palette-games"],
    queryFn: () => api.listGames({ limit: 200, sort: "title", order: "asc" }),
    enabled: open,
    staleTime: 60_000,
  });

  useEffect(() => {
    if (!open) return;
    setQuery("");
    setActive(0);
    const timer = setTimeout(() => inputRef.current?.focus(), 30);
    return () => clearTimeout(timer);
  }, [open]);

  const close = () => onOpenChange(false);

  const sections = useMemo<Section[]>(() => {
    const navContext: NavContext = { editor: isEditor, admin: isAdmin, desktop };
    const q = query.trim();
    const go = (path: string) => () => {
      close();
      navigate(path);
    };

    const destinationItem = (destination: NavDestination): Item => ({
      id: `nav-${destination.id}`,
      label: navLabel(destination),
      icon: <NavIcon name={destination.icon} className="size-3.5" />,
      kbd: destination.kbd,
      run: go(destination.to),
    });

    // The rail and the palette share one table (#174); the palette adds the
    // settings categories, which are places inside one screen.
    const navigation = navGroups(navContext).map((entry) => ({
      id: entry.group.id,
      label: navGroupLabel(entry.group.id),
      items: entry.items.map(destinationItem),
    }));

    const settingsSections = availableSettingsSections({ desktop, native: isNative() });
    const settings: Section = {
      id: "settings",
      label: t("Settings"),
      items: [
        {
          id: "set-open",
          label: t("Open settings"),
          hint: t("Every section"),
          icon: <Settings className="size-3.5" />,
          kbd: systemNav(navContext)[0]?.kbd,
          keywords: ["preferences", "options"],
          run: go("/settings"),
        },
        ...settingsSections.map((section) => ({
          id: `set-${section.id}`,
          label: settingsSectionLabel(section.id),
          hint: t("Settings section"),
          icon: <SettingsIcon name={section.icon} className="size-3.5" />,
          keywords: [...section.keywords, section.id, "settings"],
          run: go(settingsSectionPath(section.id)),
        })),
      ],
    };

    const library: Section = {
      id: "library-actions",
      label: t("Library actions"),
      items: [
        {
          id: "lib-filters",
          label: t("Open the filter panel"),
          icon: <SlidersHorizontal className="size-3.5" />,
          kbd: "f",
          run: () => {
            close();
            navigate("/library");
            window.setTimeout(openFiltersPanel, 80);
          },
        },
        {
          id: "lib-index",
          label: t("All games"),
          hint: t("A\u2013Z list"),
          icon: <ListOrdered className="size-3.5" />,
          kbd: "g i",
          run: go("/all"),
        },
        ...(desktop
          ? [
              {
                id: "lib-installed",
                label: t("Show installed games"),
                icon: <Gamepad2 className="size-3.5" />,
                run: go("/library?installed=true"),
              },
            ]
          : []),
        {
          id: "lib-clear",
          label: t("Clear library filters"),
          icon: <SlidersHorizontal className="size-3.5" />,
          run: go("/library"),
        },
      ],
    };

    const actions: Section = {
      id: "actions",
      label: t("Actions"),
      items: [
        {
          id: "act-random",
          label: t("Pick a random game"),
          icon: <Dice5 className="size-3.5" />,
          run: () => {
            close();
            void api
              .randomGame()
              .then((game) => navigate(`/game/${game.id}`))
              .catch(() => push({ title: t("Nothing to pick"), description: t("The library is empty.") }));
          },
        },
        ...(isEditor
          ? [
              {
                id: "act-scan",
                label: t("Scan the library now"),
                icon: <RefreshCw className="size-3.5" />,
                run: () => {
                  close();
                  void api
                    .startScan()
                    .then(() => push({ title: t("Scan started"), description: t("New files will appear as they are indexed."), tone: "success" as const }))
                    .catch((error: Error) => push({ title: t("Scan did not start"), description: error.message, tone: "danger" as const }));
                },
              },
              {
                id: "act-add",
                label: t("Add a game"),
                hint: t("Paste a download URL"),
                icon: <Plus className="size-3.5" />,
                run: go("/admin/ingest"),
              },
            ]
          : []),
        {
          id: "act-theme",
          label: resolved === "dark" ? t("Switch to the light theme") : t("Switch to the dark theme"),
          icon: resolved === "dark" ? <Sun className="size-3.5" /> : <Moon className="size-3.5" />,
          run: () => {
            close();
            setTheme(otherModeTheme(resolved));
          },
        },
        {
          id: "act-shortcuts",
          label: t("Keyboard shortcuts"),
          hint: t("Every shortcut"),
          icon: <Keyboard className="size-3.5" />,
          kbd: "?",
          run: () => {
            close();
            openShortcutsHelp();
          },
        },
        {
          id: "act-signout",
          label: t("Sign out"),
          icon: <UserRound className="size-3.5" />,
          run: () => {
            close();
            void signOut();
          },
        },
      ],
    };

    const games: Section = {
      id: "games",
      label: q ? t("Games") : t("Jump back in"),
      items: (data?.items ?? [])
        .map((game) => ({ game, score: q ? scoreSearch(q, game.title, game.genres) : 1 }))
        .filter((entry) => (q ? entry.score > 0 : entry.game.lastPlayedAt !== null))
        .sort((a, b) => (q ? b.score - a.score : 0))
        .slice(0, q ? 7 : 4)
        .map(({ game }) => ({
          id: `game-${game.id}`,
          label: game.title,
          hint: `${formatYear(game.releaseDate)} / ${stateLabel(game.state)}`,
          icon: (
            <span className="block w-[30px] overflow-hidden rounded-[1px] ring-1 ring-sheen/15">
              <GameCover title={game.title} coverUrl={game.coverUrl} showType={false} />
            </span>
          ),
          run: () => {
            close();
            navigate(`/game/${game.id}`);
          },
        })),
    };

    const all = [games, ...navigation, settings, library, actions].filter((section) => section.items.length > 0);
    if (!q) return all;

    return all
      .map((section) => ({
        ...section,
        items:
          section.id === "games"
            ? section.items
            : section.items
                .map((item) => ({
                  item,
                  score: scoreSearch(q, item.label, [...(item.keywords ?? []), section.label]),
                }))
                .filter((entry) => entry.score > 0)
                .sort((a, b) => b.score - a.score)
                .map((entry) => entry.item),
      }))
      .filter((section) => section.items.length > 0);
  }, [data, query, navigate, isEditor, isAdmin, desktop, resolved, setTheme, signOut, push]);

  const items = useMemo(() => sections.flatMap((section) => section.items), [sections]);

  useEffect(() => {
    setActive(0);
  }, [query]);

  useEffect(() => {
    if (!open) return;
    const el = listRef.current?.querySelector<HTMLElement>(`[data-index="${active}"]`);
    el?.scrollIntoView({ block: "nearest" });
  }, [active, open]);

  const onKeyDown = (event: React.KeyboardEvent) => {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setActive((i) => (items.length ? (i + 1) % items.length : 0));
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setActive((i) => (items.length ? (i - 1 + items.length) % items.length : 0));
    } else if (event.key === "Home") {
      event.preventDefault();
      setActive(0);
    } else if (event.key === "End") {
      event.preventDefault();
      setActive(Math.max(0, items.length - 1));
    } else if (event.key === "Enter") {
      event.preventDefault();
      items[active]?.run();
    } else if (event.key === "Escape") {
      event.preventDefault();
      close();
    } else if (event.key === "Tab") {
      event.preventDefault();
    }
  };

  let index = -1;

  return createPortal(
    <AnimatePresence>
      {open && (
        <motion.div
          className="fixed inset-0 z-[95] flex items-start justify-center px-4 pt-[10vh]"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: reduce ? 0 : 0.14 }}
        >
          <div className="fixed inset-0 scrim" onClick={close} aria-hidden="true" />
          <motion.div
            role="dialog"
            aria-modal="true"
            aria-label={t("Search and commands")}
            initial={reduce ? false : { opacity: 0, y: -6, scale: 0.995 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={reduce ? { opacity: 0 } : { opacity: 0, y: -4 }}
            transition={{ duration: reduce ? 0 : 0.18, ease: [0.16, 1, 0.3, 1] }}
            className="relative z-10 w-full max-w-[640px] overflow-hidden rounded-[3px] border border-line-2 bg-surface shadow-palette"
            onKeyDown={onKeyDown}
          >
            <div className="flex items-center gap-3 border-b border-line px-4">
              <Search className="size-4 shrink-0 text-ink-3" aria-hidden="true" />
              <input
                ref={inputRef}
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                placeholder={t("Search games, jump to a screen, run a command")}
                aria-label={t("Search games and commands")}
                aria-controls={listId}
                aria-activedescendant={items[active]?.id}
                role="combobox"
                aria-expanded="true"
                aria-autocomplete="list"
                autoComplete="off"
                spellCheck={false}
                className="h-[54px] w-full bg-transparent text-[15px] text-ink outline-none placeholder:text-ink-3"
              />
              <Kbd>esc</Kbd>
            </div>

            <div ref={listRef} id={listId} role="listbox" aria-label={t("Results")} className="max-h-[min(56vh,440px)] overflow-y-auto py-1">
              {items.length === 0 && (
                <div className="px-4 py-8 text-center text-[13px] text-ink-3">
                  {t("Nothing matches “{query}”. Try a title, a genre or a screen name.", { query })}
                </div>
              )}
              {sections.map((section) => {
                const groupId = `${groupIdPrefix}-${section.id}`;
                return (
                  <div key={section.id} role="group" aria-labelledby={groupId}>
                    <div id={groupId} className="px-4 pb-1 pt-3 text-[11px] font-medium text-ink-3">
                      {section.label}
                    </div>
                    {section.items.map((item) => {
                      index += 1;
                      const position = index;
                      return (
                        <button
                          key={item.id}
                          type="button"
                          role="option"
                          id={item.id}
                          data-index={position}
                          aria-selected={position === active}
                          onMouseMove={() => setActive(position)}
                          onClick={item.run}
                          className={cn(
                            "flex h-11 w-full items-center gap-3 px-4 text-left transition-colors duration-[80ms]",
                            position === active ? "bg-surface-2 text-ink" : "text-ink-2",
                          )}
                        >
                          <span className="flex w-8 shrink-0 items-center justify-center text-ink-3">{item.icon}</span>
                          <span className="min-w-0 flex-1 truncate text-[13.5px]">{item.label}</span>
                          {item.hint && <span className="hidden shrink-0 text-[11.5px] text-ink-3 sm:block">{item.hint}</span>}
                          {item.kbd && <span className="kbd hidden sm:inline-flex">{item.kbd}</span>}
                          <ArrowRight
                            className={cn("size-3.5 shrink-0", position === active ? "text-signal-text" : "text-transparent")}
                            aria-hidden="true"
                          />
                        </button>
                      );
                    })}
                  </div>
                );
              })}
            </div>

            <div className="flex items-center justify-between gap-4 border-t border-line px-4 py-2.5 text-[11.5px] text-ink-3">
              <span className="flex items-center gap-3">
                <span className="flex items-center gap-1.5">
                  <Kbd>↑</Kbd>
                  <Kbd>↓</Kbd>
                  {t("to move")}
                </span>
                <span className="flex items-center gap-1.5">
                  <Kbd>↵</Kbd>
                  {t("to open")}
                </span>
              </span>
              <span className="flex items-center gap-1.5">
                <Kbd>/</Kbd>
                {t("from anywhere")}
              </span>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>,
    document.body,
  );
}
