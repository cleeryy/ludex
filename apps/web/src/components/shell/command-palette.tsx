import { useEffect, useId, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useNavigate } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import {
  ArrowRight,
  Bookmark,
  Clock,
  Dice5,
  Download,
  Film,
  Library,
  ListOrdered,
  Moon,
  Plus,
  RefreshCw,
  Search,
  Server,
  Settings,
  ShieldCheck,
  Sun,
  UserRound,
  Users,
} from "lucide-react";
import { api } from "@/lib/api";
import { cn } from "@/lib/cn";
import { formatYear, stateLabel } from "@/lib/format";
import { useAuth, useIsAdmin, useIsEditor } from "@/state/auth";
import { useSettings } from "@/state/settings";
import { useToast } from "@/state/toasts";
import { GameCover } from "@/components/art/cover";
import { Kbd } from "@/components/ui/controls";

interface Item {
  id: string;
  group: string;
  label: string;
  hint?: string;
  icon?: React.ReactNode;
  kbd?: string;
  run: () => void;
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

  const items = useMemo<Item[]>(() => {
    const go = (path: string) => () => {
      close();
      navigate(path);
    };

    const navigation: Item[] = [
      { id: "nav-library", group: "Go to", label: "Library", icon: <Library className="size-3.5" />, run: go("/") },
      { id: "nav-recent", group: "Go to", label: "Recently played", icon: <Clock className="size-3.5" />, run: go("/recent") },
      { id: "nav-bookmarks", group: "Go to", label: "Bookmarks", icon: <Bookmark className="size-3.5" />, run: go("/bookmarks") },
      { id: "nav-index", group: "Go to", label: "Index", hint: "A–Z list", icon: <ListOrdered className="size-3.5" />, run: go("/all") },
      { id: "nav-news", group: "Go to", label: "News", icon: <Film className="size-3.5" />, run: go("/news") },
      { id: "nav-settings", group: "Go to", label: "Settings", icon: <Settings className="size-3.5" />, run: go("/settings") },
    ];

    const admin: Item[] = isEditor
      ? [
          { id: "adm-ingest", group: "Admin", label: "Ingest", hint: "Add a download", icon: <Download className="size-3.5" />, run: go("/admin/ingest") },
          { id: "adm-scan", group: "Admin", label: "Library scan", icon: <RefreshCw className="size-3.5" />, run: go("/admin/scan") },
          ...(isAdmin
            ? [
                { id: "adm-users", group: "Admin", label: "Users", icon: <Users className="size-3.5" />, run: go("/admin/users") },
                { id: "adm-news", group: "Admin", label: "News editor", icon: <ShieldCheck className="size-3.5" />, run: go("/admin/news") },
                { id: "adm-server", group: "Admin", label: "Server", icon: <Server className="size-3.5" />, run: go("/admin/server") },
              ]
            : []),
        ]
      : [];

    const actions: Item[] = [
      {
        id: "act-random",
        group: "Actions",
        label: "Pick a random game",
        icon: <Dice5 className="size-3.5" />,
        run: () => {
          close();
          void api
            .randomGame()
            .then((game) => navigate(`/game/${game.id}`))
            .catch(() => push({ title: "Nothing to pick", description: "The library is empty." }));
        },
      },
      ...(isEditor
        ? [
            {
              id: "act-scan",
              group: "Actions",
              label: "Scan the library now",
              icon: <RefreshCw className="size-3.5" />,
              run: () => {
                close();
                void api
                  .startScan()
                  .then(() => push({ title: "Scan started", description: "New files will appear as they are indexed.", tone: "success" }))
                  .catch((error: Error) => push({ title: "Scan did not start", description: error.message, tone: "danger" }));
              },
            },
            {
              id: "act-add",
              group: "Actions",
              label: "Add a game",
              hint: "Paste a download URL",
              icon: <Plus className="size-3.5" />,
              run: go("/admin/ingest"),
            },
          ]
        : []),
      {
        id: "act-theme",
        group: "Actions",
        label: resolved === "dark" ? "Switch to the light theme" : "Switch to the dark theme",
        icon: resolved === "dark" ? <Sun className="size-3.5" /> : <Moon className="size-3.5" />,
        run: () => {
          close();
          setTheme(resolved === "dark" ? "light" : "dark");
        },
      },
      {
        id: "act-signout",
        group: "Actions",
        label: "Sign out",
        icon: <UserRound className="size-3.5" />,
        run: () => {
          close();
          void signOut();
        },
      },
    ];

    const q = query.trim().toLowerCase();
    const games: Item[] = (data?.items ?? [])
      .filter((game) => {
        if (!q) return game.lastPlayedAt !== null;
        return (
          game.title.toLowerCase().includes(q) ||
          game.genres.some((genre) => genre.toLowerCase().includes(q))
        );
      })
      .slice(0, q ? 7 : 4)
      .map((game) => ({
        id: `game-${game.id}`,
        group: q ? "Games" : "Jump back in",
        label: game.title,
        hint: `${formatYear(game.releaseDate)} / ${stateLabel(game.state)}`,
        icon: (
          <span className="block w-[30px] overflow-hidden rounded-[1px] ring-1 ring-white/15">
            <GameCover title={game.title} coverUrl={game.coverUrl} showType={false} />
          </span>
        ),
        run: () => {
          close();
          navigate(`/game/${game.id}`);
        },
      }));

    const groups = [...games, ...navigation, ...admin, ...actions];
    if (!q) return groups;
    return groups.filter((item) => {
      if (item.group === "Games") return true;
      return item.label.toLowerCase().includes(q) || item.group.toLowerCase().includes(q);
    });
  }, [data, query, navigate, isEditor, isAdmin, resolved, setTheme, signOut, push]);

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
          <div className="fixed inset-0 bg-[var(--scrim)]" onClick={close} aria-hidden="true" />
          <motion.div
            role="dialog"
            aria-modal="true"
            aria-label="Search and commands"
            initial={reduce ? false : { opacity: 0, y: -6, scale: 0.995 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={reduce ? { opacity: 0 } : { opacity: 0, y: -4 }}
            transition={{ duration: reduce ? 0 : 0.18, ease: [0.16, 1, 0.3, 1] }}
            className="relative z-10 w-full max-w-[640px] overflow-hidden rounded-[3px] border border-line-2 bg-surface shadow-[0_40px_120px_-30px_rgba(0,0,0,0.8)]"
            onKeyDown={onKeyDown}
          >
            <div className="flex items-center gap-3 border-b border-line px-4">
              <Search className="size-4 shrink-0 text-ink-3" aria-hidden="true" />
              <input
                ref={inputRef}
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                placeholder="Search games, jump to a screen, run a command"
                aria-label="Search games and commands"
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

            <div ref={listRef} id={listId} role="listbox" aria-label="Results" className="max-h-[min(56vh,440px)] overflow-y-auto py-1">
              {items.length === 0 && (
                <div className="px-4 py-8 text-center text-[13px] text-ink-3">
                  Nothing matches “{query}”. Try a title, a genre or a screen name.
                </div>
              )}
              {items.map((item, index) => {
                const showGroup = index === 0 || items[index - 1].group !== item.group;
                return (
                  <div key={item.id}>
                    {showGroup && (
                      <div className="px-4 pb-1 pt-3 text-[11px] font-medium text-ink-3" aria-hidden="true">
                        {item.group}
                      </div>
                    )}
                    <button
                      type="button"
                      role="option"
                      id={item.id}
                      data-index={index}
                      aria-selected={index === active}
                      onMouseMove={() => setActive(index)}
                      onClick={item.run}
                      className={cn(
                        "flex h-11 w-full items-center gap-3 px-4 text-left transition-colors duration-[80ms]",
                        index === active ? "bg-surface-2 text-ink" : "text-ink-2",
                      )}
                    >
                      <span className="flex w-8 shrink-0 items-center justify-center text-ink-3">{item.icon}</span>
                      <span className="min-w-0 flex-1 truncate text-[13.5px]">{item.label}</span>
                      {item.hint && <span className="hidden shrink-0 text-[11.5px] text-ink-3 sm:block">{item.hint}</span>}
                      {item.kbd && <span className="kbd hidden sm:inline-flex">{item.kbd}</span>}
                      <ArrowRight
                        className={cn("size-3.5 shrink-0", index === active ? "text-signal-text" : "text-transparent")}
                        aria-hidden="true"
                      />
                    </button>
                  </div>
                );
              })}
            </div>

            <div className="flex items-center justify-between gap-4 border-t border-line px-4 py-2.5 text-[11.5px] text-ink-3">
              <span className="flex items-center gap-3">
                <span className="flex items-center gap-1.5">
                  <Kbd>↑</Kbd>
                  <Kbd>↓</Kbd>
                  to move
                </span>
                <span className="flex items-center gap-1.5">
                  <Kbd>↵</Kbd>
                  to open
                </span>
              </span>
              <span className="flex items-center gap-1.5">
                <Kbd>/</Kbd>
                from anywhere
              </span>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>,
    document.body,
  );
}
