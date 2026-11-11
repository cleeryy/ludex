import { useEffect, useId, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { Loader2, Search, X } from "lucide-react";
import { api } from "@/lib/api";
import { cn } from "@/lib/cn";
import { consumeSearchFocus, FOCUS_SEARCH_EVENT } from "@/lib/ui-events";
import { highlightRange } from "@/lib/search";
import type { GameSuggestion } from "@/lib/types";
import { GameCover } from "@/components/art/cover";
import { t } from "@/lib/i18n";

function Highlight({ text, query }: { text: string; query: string }) {
  const range = highlightRange(text, query);
  if (!range) return <>{text}</>;
  const [start, end] = range;
  return (
    <>
      {text.slice(0, start)}
      <mark className="rounded-[1px] bg-signal/25 text-ink">{text.slice(start, end)}</mark>
      {text.slice(end)}
    </>
  );
}

/**
 * Library search with debounced, server-ranked suggestions. The input is a
 * combobox: arrows move through the list, Enter opens the active suggestion or
 * commits the query, Escape closes the dropdown.
 */
export function LibrarySearch({
  value,
  onCommit,
  className,
}: {
  value: string;
  onCommit: (value: string) => void;
  className?: string;
}) {
  const navigate = useNavigate();
  const [text, setText] = useState(value);
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(-1);
  const [debounced, setDebounced] = useState(value);
  const inputRef = useRef<HTMLInputElement>(null);
  const listId = useId();
  const optionId = (index: number) => `${listId}-option-${index}`;

  // External changes (chips, clear-all, URL restore) win over local typing.
  useEffect(() => {
    setText(value);
  }, [value]);

  useEffect(() => {
    const timer = setTimeout(() => setDebounced(text), 170);
    return () => clearTimeout(timer);
  }, [text]);

  useEffect(() => {
    const timer = setTimeout(() => onCommit(text), 260);
    return () => clearTimeout(timer);
    // onCommit is recreated every render by the screen; text is the real input.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [text]);

  useEffect(() => {
    if (consumeSearchFocus()) inputRef.current?.focus();
    const onFocusRequest = () => {
      consumeSearchFocus();
      inputRef.current?.focus();
      inputRef.current?.select();
    };
    window.addEventListener(FOCUS_SEARCH_EVENT, onFocusRequest);
    return () => window.removeEventListener(FOCUS_SEARCH_EVENT, onFocusRequest);
  }, []);

  const query = debounced.trim();
  const { data, isFetching } = useQuery({
    queryKey: ["suggest", query],
    queryFn: () => api.suggestGames(query, 8),
    enabled: open && query.length >= 2,
    staleTime: 30_000,
  });

  const suggestions: GameSuggestion[] = query.length >= 2 ? (data?.suggestions ?? []) : [];
  const showDropdown = open && text.trim().length >= 2;
  const showEmpty = showDropdown && !isFetching && suggestions.length === 0;

  useEffect(() => {
    setActive(-1);
  }, [query]);

  const commit = (next: string) => {
    setOpen(false);
    setActive(-1);
    onCommit(next);
  };

  const choose = (suggestion: GameSuggestion) => {
    setOpen(false);
    navigate(`/game/${suggestion.id}`);
  };

  const onKeyDown = (event: React.KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setOpen(true);
      if (suggestions.length) setActive((current) => (current + 1) % suggestions.length);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      if (suggestions.length) setActive((current) => (current - 1 + suggestions.length) % suggestions.length);
    } else if (event.key === "Enter") {
      event.preventDefault();
      if (active >= 0 && suggestions[active]) choose(suggestions[active]);
      else commit(text);
    } else if (event.key === "Escape") {
      if (showDropdown) {
        event.preventDefault();
        setOpen(false);
        setActive(-1);
      }
    } else if (event.key === "Tab") {
      setOpen(false);
    }
  };

  return (
    <div className={cn("relative", className)}>
      <div className="flex h-9 items-center gap-2 rounded-[2px] border border-line bg-surface px-2.5 transition-colors focus-within:border-line-2">
        <Search className="size-3.5 shrink-0 text-ink-3" aria-hidden="true" />
        <input
          ref={inputRef}
          value={text}
          onChange={(event) => {
            setText(event.target.value);
            setOpen(true);
          }}
          onFocus={() => setOpen(true)}
          onBlur={() => setOpen(false)}
          onKeyDown={onKeyDown}
          placeholder={t("Search titles")}
          aria-label={t("Search the library")}
          role="combobox"
          aria-expanded={showDropdown}
          aria-controls={showDropdown ? listId : undefined}
          aria-autocomplete="list"
          aria-activedescendant={active >= 0 ? optionId(active) : undefined}
          autoComplete="off"
          spellCheck={false}
          className="h-full w-full min-w-0 bg-transparent text-[13px] text-ink outline-none placeholder:text-ink-3"
        />
        {isFetching && showDropdown ? (
          <Loader2 className="size-3.5 shrink-0 animate-spin text-ink-3" aria-hidden="true" />
        ) : text ? (
          <button
            type="button"
            onClick={() => {
              setText("");
              commit("");
              inputRef.current?.focus();
            }}
            aria-label={t("Clear search")}
            className="grid size-5 shrink-0 place-items-center rounded-[2px] text-ink-3 transition-colors hover:text-ink"
          >
            <X className="size-3.5" aria-hidden="true" />
          </button>
        ) : (
          <span className="kbd hidden shrink-0 sm:inline-flex">/</span>
        )}
      </div>

      {showDropdown && (
        <div className="absolute inset-x-0 top-[calc(100%+4px)] z-40 overflow-hidden rounded-[2px] border border-line-2 bg-surface shadow-pop">
          <div id={listId} role="listbox" aria-label={t("Search suggestions")} className="max-h-[320px] overflow-y-auto py-1">
            {suggestions.map((suggestion, index) => (
              <button
                key={suggestion.id}
                id={optionId(index)}
                type="button"
                role="option"
                aria-selected={index === active}
                onMouseDown={(event) => event.preventDefault()}
                onMouseEnter={() => setActive(index)}
                onClick={() => choose(suggestion)}
                className={cn(
                  "flex w-full items-center gap-3 px-3 py-2 text-left transition-colors duration-[80ms]",
                  index === active ? "bg-surface-2 text-ink" : "text-ink-2",
                )}
              >
                <span className="block w-7 shrink-0 overflow-hidden rounded-[1px] ring-1 ring-line">
                  <GameCover title={suggestion.title} coverUrl={suggestion.coverUrl} showType={false} />
                </span>
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-[13px]">
                    <Highlight text={suggestion.title} query={text} />
                  </span>
                  <span className="block truncate font-mono text-[10px] text-ink-3">
                    {suggestion.year ?? "—"}
                    {suggestion.matchedField === "altTitle" ? ` / alias: ${suggestion.matchedTitle}` : ""}
                  </span>
                </span>
                {suggestion.matchedField !== "title" && (
                  <span className="shrink-0 font-mono text-[9.5px] uppercase tracking-[0.08em] text-ink-3">
                    {suggestion.matchedField === "altTitle" ? "alias" : "sort"}
                  </span>
                )}
              </button>
            ))}
            {showEmpty && (
              <p className="px-3 py-6 text-center text-[12.5px] text-ink-3">
                {t("No suggestion for “{query}”. Press Enter to search anyway.", { query: text.trim() })}
              </p>
            )}
            {suggestions.length > 0 && (
              <button
                type="button"
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => commit(text)}
                className="mt-1 flex w-full items-center gap-2 border-t border-line px-3 py-2 text-left text-[11.5px] text-ink-3 transition-colors hover:text-ink"
              >
                <Search className="size-3" aria-hidden="true" />
                {t("Search all titles for “{query}”", { query: text.trim() })}
              </button>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
