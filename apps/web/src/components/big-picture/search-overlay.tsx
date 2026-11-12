import { Search as SearchIcon } from "lucide-react";
import { motion, useReducedMotion } from "motion/react";
import type { RefObject } from "react";
import { cn } from "@/lib/cn";
import { formatYear } from "@/lib/format";
import { t } from "@/lib/i18n";
import { OverlayShell } from "./overlays";
import {
  SEARCH_KEY_ROWS,
  flattenSearchRows,
  searchKeyLabel,
  type SearchCell,
} from "./search";

/* --------------------------------------------------------------------------
   The couch search overlay (#138).

   A real input keeps physical typing (and screen readers) working, while the
   on-screen QWERTY and the result list below it share one focus grid: the
   d-pad walks letters then results, A presses the highlighted cell, X deletes,
   Y spaces, LB clears and RB opens the first match. The parent debounces the
   query and owns the highlight index, so this component stays a renderer.
   -------------------------------------------------------------------------- */

export function SearchOverlay({
  inputRef,
  query,
  onQuery,
  rows,
  active,
  onChoose,
  onHighlight,
  ...shell
}: {
  inputRef: RefObject<HTMLInputElement | null>;
  query: string;
  onQuery: (value: string) => void;
  rows: readonly SearchCell[][];
  active: number;
  onChoose: (index: number) => void;
  onHighlight: (index: number) => void;
} & Omit<Parameters<typeof OverlayShell>[0], "children" | "title" | "subtitle">) {
  const cells = flattenSearchRows(rows);
  const keys = rows.slice(0, SEARCH_KEY_ROWS.length);
  const results = rows.slice(SEARCH_KEY_ROWS.length).flat();
  const resultOffset = keys.reduce((total, row) => total + row.length, 0);
  const reduce = useReducedMotion();

  return (
    <OverlayShell
      title={t("Search the library")}
      subtitle={t("Type with the on-screen keyboard or your keyboard. A presses the highlighted key.")}
      wide
      {...shell}
    >
      <div className="flex items-center gap-2 border border-line bg-bg/60 px-3 py-2">
        <SearchIcon className="size-4 shrink-0 text-ink-3" aria-hidden="true" />
        <input
          ref={inputRef}
          value={query}
          onChange={(event) => onQuery(event.target.value)}
          placeholder={t("Type a title")}
          aria-label={t("Search the library")}
          autoComplete="off"
          autoCorrect="off"
          spellCheck={false}
          className="min-w-0 flex-1 bg-transparent text-[15px] text-ink outline-none placeholder:text-ink-3"
        />
        <span className="num shrink-0 font-mono text-[10.5px] text-ink-3">{results.length}</span>
      </div>

      <div role="group" aria-label={t("On-screen keyboard")} className="mt-2 flex flex-col gap-1">
        {keys.map((row, rowIndex) => (
          <div key={rowIndex} className="flex items-stretch justify-center gap-1">
            {row.map((cell) => {
              if (cell.kind !== "key") return null;
              const index = cells.indexOf(cell);
              const special = cell.key === "backspace" || cell.key === "space" || cell.key === "clear";
              return (
                <motion.button
                  key={cell.key}
                  type="button"
                  data-overlay-active={index === active ? "true" : undefined}
                  data-gamepad-focus={index === active ? "true" : undefined}
                  onClick={() => onChoose(index)}
                  onPointerEnter={() => onHighlight(index)}
                  whileTap={{ scale: reduce ? 1 : 0.96 }}
                  transition={{ duration: reduce ? 0 : 0.08 }}
                  className={cn(
                    "h-10 min-w-10 border px-2 font-mono text-[13px] transition-colors duration-100",
                    special && "flex-1 text-[12px]",
                    index === active
                      ? "border-signal bg-signal/20 text-ink"
                      : "border-line bg-bg/40 text-ink-2 hover:border-line-2 hover:text-ink",
                  )}
                  aria-label={searchKeyLabel(cell.key, (key) => t(key === "space" ? "space" : "clear"))}
                >
                  {searchKeyLabel(cell.key, (key) => t(key === "space" ? "space" : "clear"))}
                </motion.button>
              );
            })}
          </div>
        ))}
      </div>

      <div className="mt-3 border-t border-line pt-2" role="group" aria-label={t("Results")}>
        {results.length === 0 ? (
          <p className="px-3 py-4 text-center text-[13px] text-ink-3">{t("No matches")}</p>
        ) : (
          <ul className="flex flex-col gap-0.5">
            {results.map((cell, position) => {
              if (cell.kind !== "result") return null;
              const index = resultOffset + position;
              return (
                <li key={cell.game.id}>
                  <button
                    type="button"
                    data-overlay-active={index === active ? "true" : undefined}
                    data-gamepad-focus={index === active ? "true" : undefined}
                    onClick={() => onChoose(index)}
                    onPointerEnter={() => onHighlight(index)}
                    className={cn(
                      "flex w-full items-center gap-3 border px-3 py-2 text-start transition-colors duration-100",
                      index === active
                        ? "border-signal bg-surface-2"
                        : "border-transparent hover:border-line hover:bg-surface/60",
                    )}
                  >
                    <span className="num w-6 shrink-0 font-mono text-[10.5px] text-ink-3">
                      {String(position + 1).padStart(2, "0")}
                    </span>
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-[13.5px] text-ink">{cell.game.title}</span>
                      <span className="num block truncate text-[10.5px] text-ink-3">
                        {formatYear(cell.game.releaseDate)}
                        {cell.game.genres[0] ? ` · ${cell.game.genres[0]}` : ""}
                      </span>
                    </span>
                  </button>
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </OverlayShell>
  );
}
