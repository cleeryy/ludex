import { useEffect, useId, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { FolderOpen, RefreshCw } from "lucide-react";
import { errorMessage } from "@/lib/http";
import { formatBytes } from "@/lib/format";
import { isNative, native } from "@/lib/native";
import type { GameExecutableKind } from "@/lib/types";
import {
  activeIndexForKey,
  clampActiveIndex,
  filterExecutables,
  pickerState,
  recommendedExecutable,
} from "@/lib/executable-picker";
import { useToast } from "@/state/toasts";
import { Badge, Button, Input } from "@/components/ui/controls";
import { cn } from "@/lib/cn";
import { t } from "@/lib/i18n";

/** Query key prefix; mutations invalidate every listing at once (#143). */
export const GAME_EXECUTABLES_QUERY = "game-executables";

/**
 * Programs found in one game's install folder (#143).
 *
 * `staleTime` matches the desktop cache: within a few seconds the backend
 * answers from its own cache, and the refresh button asks for a real rescan.
 */
export function useGameExecutables(gameId: number, enabled = true) {
  return useQuery({
    queryKey: [GAME_EXECUTABLES_QUERY, gameId],
    queryFn: () => native.listGameExecutables(gameId),
    enabled: enabled && isNative() && Number.isFinite(gameId),
    staleTime: 4_000,
    retry: false,
  });
}

function kindTitle(kind: GameExecutableKind): string {
  switch (kind) {
    case "installer":
      return t("Installer");
    case "uninstaller":
      return t("Uninstaller");
    case "other":
      return t("Other programs");
    default:
      return t("Executable");
  }
}

/**
 * Searchable list of programs found in the install folder, with manual entry as
 * an advanced fallback.
 *
 * Keyboard navigation follows the combobox pattern: the search input owns the
 * cursor (`aria-activedescendant`) and Arrow/Home/End move it while Enter
 * selects. The visible list is already filtered by kind, so the installer
 * picker never offers the game binary.
 */
export function ExecutableField({
  gameId,
  kind,
  value,
  onChange,
  label,
  hint,
  autoSelectRecommended = false,
  disabled = false,
}: {
  gameId: number;
  kind: GameExecutableKind;
  value: string;
  onChange: (value: string) => void;
  /** Visible label; defaults to the kind name. */
  label?: string;
  hint?: string;
  /** Pre-select the detected executable when nothing is recorded yet. */
  autoSelectRecommended?: boolean;
  disabled?: boolean;
}) {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const query = useGameExecutables(gameId, !disabled);
  const [search, setSearch] = useState("");
  const [activeIndex, setActiveIndex] = useState(-1);
  const [manualOpen, setManualOpen] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const autoSelected = useRef(false);
  const listId = useId();

  const title = label ?? kindTitle(kind);
  const entries = query.data?.entries ?? [];
  const options = useMemo(
    () => filterExecutables(entries, kind, search),
    [entries, kind, search],
  );
  const state = pickerState({
    isLoading: query.isPending,
    error: query.error,
    entries: query.data?.entries,
    kind,
  });
  const activeId = activeIndex >= 0 ? `${listId}-${activeIndex}` : undefined;

  useEffect(() => {
    setActiveIndex((current) => clampActiveIndex(current, options.length));
  }, [options.length]);

  // The acceptance criterion for #143: the detected main executable is
  // preselected when the game has no recorded one yet.
  useEffect(() => {
    if (!autoSelectRecommended || value || autoSelected.current || state !== "ready") return;
    const detected = recommendedExecutable(entries, kind);
    if (detected) {
      autoSelected.current = true;
      onChange(detected.path);
    }
  }, [autoSelectRecommended, value, state, entries, kind, onChange]);

  const select = (path: string) => {
    onChange(path);
    setSearch("");
    setActiveIndex(-1);
  };

  const refresh = async () => {
    setRefreshing(true);
    try {
      const next = await native.listGameExecutables(gameId, true);
      queryClient.setQueryData([GAME_EXECUTABLES_QUERY, gameId], next);
    } catch (cause) {
      push({
        title: t("Could not rescan the install folder"),
        description: errorMessage(cause),
        tone: "danger",
      });
    } finally {
      setRefreshing(false);
    }
  };

  const browse = async () => {
    const folder = query.data?.installPath;
    if (!folder) return;
    try {
      await native.openPath(folder);
    } catch (cause) {
      push({
        title: t("Could not open the install folder"),
        description: errorMessage(cause),
        tone: "danger",
      });
    }
  };

  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Enter") {
      const option = activeIndex >= 0 ? options[activeIndex] : options[0];
      if (option) {
        event.preventDefault();
        select(option.path);
      }
      return;
    }
    if (event.key === "Escape") {
      setSearch("");
      setActiveIndex(-1);
      return;
    }
    const next = activeIndexForKey(activeIndex, event.key, options.length);
    if (next !== null) {
      event.preventDefault();
      setActiveIndex(next);
    }
  };

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <span className="text-[12px] text-ink-2">{title}</span>
        <span className="font-mono text-[10.5px] text-ink-3">
          {value ? value : t("Nothing selected")}
        </span>
      </div>

      <div className="flex items-center gap-2">
        <Input
          value={search}
          onChange={(event) => setSearch(event.target.value)}
          onKeyDown={onKeyDown}
          disabled={disabled}
          role="combobox"
          aria-label={t("Search in the install folder")}
          aria-autocomplete="list"
          aria-expanded={options.length > 0}
          aria-controls={options.length > 0 ? listId : undefined}
          aria-activedescendant={activeId}
          placeholder={t("Search in the install folder")}
          spellCheck={false}
          className="h-10 flex-1 font-mono text-[12px]"
        />
        <Button
          variant="ghost"
          size="sm"
          className="h-10 shrink-0"
          icon={<RefreshCw className="size-3.5" />}
          loading={refreshing}
          disabled={disabled}
          aria-label={t("Rescan the install folder")}
          onClick={() => void refresh()}
        >
          {t("Refresh")}
        </Button>
        <Button
          variant="default"
          size="sm"
          className="h-10 shrink-0"
          icon={<FolderOpen className="size-3.5" />}
          disabled={disabled || !query.data?.installPath}
          onClick={() => void browse()}
        >
          {t("Browse folder")}
        </Button>
      </div>

      {state === "loading" && (
        <p className="text-[11.5px] text-ink-3">{t("Scanning the install folder…")}</p>
      )}
      {state === "error" && (
        <p role="alert" className="border-l-2 border-danger pl-3 text-[11.5px] leading-relaxed text-danger">
          {errorMessage(query.error)}
        </p>
      )}
      {state === "empty" && (
        <p className="border-l-2 border-signal/60 pl-3 text-[11.5px] leading-relaxed text-ink-3">
          {kind === "launch"
            ? t("No executable found — choose one. Enter a relative path or browse the install folder.")
            : t("No matching program found in the install folder.")}
        </p>
      )}

      {options.length > 0 && (
        <ul
          id={listId}
          role="listbox"
          aria-label={title}
          className="max-h-52 overflow-y-auto border-y border-line"
        >
          {options.map((entry, index) => (
            <li
              key={entry.path}
              id={`${listId}-${index}`}
              role="option"
              aria-selected={value === entry.path}
              tabIndex={-1}
              onMouseEnter={() => setActiveIndex(index)}
              onClick={() => select(entry.path)}
              className={cn(
                "flex cursor-pointer items-center gap-3 border-b border-line px-2 py-1.5 transition-colors duration-[100ms]",
                activeIndex === index && "bg-surface-2",
                value === entry.path && "bg-signal/5",
              )}
            >
              <span
                className={cn(
                  "size-2 shrink-0 rounded-full border",
                  value === entry.path ? "border-signal bg-signal" : "border-line-2",
                )}
                aria-hidden="true"
              />
              <span className="min-w-0 flex-1">
                <span className="flex items-center gap-2">
                  <span className="truncate text-[12.5px] text-ink">{entry.name}</span>
                  {entry.recommended && <Badge tone="signal">{t("Detected")}</Badge>}
                </span>
                <span className="block truncate font-mono text-[10.5px] text-ink-3">
                  {entry.path}
                </span>
              </span>
              <span className="num shrink-0 text-[10.5px] text-ink-3">
                {formatBytes(entry.sizeBytes)}
              </span>
            </li>
          ))}
        </ul>
      )}

      {hint && <span className="text-[11px] text-ink-3">{hint}</span>}

      <div>
        <button
          type="button"
          onClick={() => setManualOpen((open) => !open)}
          aria-expanded={manualOpen}
          className="text-[11.5px] text-ink-3 underline-offset-4 transition-colors hover:text-ink hover:underline"
        >
          {manualOpen ? t("Hide manual entry") : t("Enter a path manually")}
        </button>
        {manualOpen && (
          <div className="mt-2 flex flex-col gap-1.5">
            <Input
              value={value}
              onChange={(event) => onChange(event.target.value)}
              disabled={disabled}
              placeholder={t("Game/Game.exe")}
              spellCheck={false}
              className="h-10 font-mono text-[12px]"
              aria-label={t("Executable path")}
            />
            <span className="text-[11px] leading-relaxed text-ink-3">
              {t("Relative to the install folder. Paths outside it are refused when saving.")}
            </span>
          </div>
        )}
      </div>
    </div>
  );
}
