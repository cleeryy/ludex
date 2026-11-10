import { useMemo, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Gamepad2, RefreshCw, Search } from "lucide-react";
import { api } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { formatBytes } from "@/lib/format";
import { isNative, native } from "@/lib/native";
import { t, tn, tr } from "@/lib/i18n";
import type { GameSummary, LauncherGame, LauncherImportEntry, LauncherPreview } from "@/lib/types";
import { useToast } from "@/state/toasts";
import { Badge, Button, Checkbox, Input } from "@/components/ui/controls";
import { Popover } from "@/components/ui/popover";
import { cn } from "@/lib/cn";

/**
 * Launcher discovery and import (#116) and the Steam library (#13).
 *
 * Ludex never downloads or copies games another launcher installed: the
 * metadata a launcher already writes on this machine is read, the user picks
 * the entries to add, and each one is recorded as an external installed game
 * with the launcher's own launch URI.
 */
export function LauncherImportPanel({
  compact = false,
  onImported,
}: {
  compact?: boolean;
  onImported?: () => void;
}) {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const [preview, setPreview] = useState<LauncherPreview | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [mapping, setMapping] = useState<Record<string, number>>({});
  const [uris, setUris] = useState<Record<string, string>>({});
  const [scanning, setScanning] = useState(false);
  const [importing, setImporting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const titles = useQuery({
    queryKey: ["library-titles"],
    queryFn: () => api.listGames({ limit: 400, sort: "title" }),
    enabled: isNative(),
    staleTime: 5 * 60_000,
  });

  const keyOf = (game: LauncherGame) => `${game.source}:${game.externalId}`;
  const byKey = useMemo(() => {
    const map = new Map<string, LauncherGame>();
    for (const game of preview?.games ?? []) map.set(keyOf(game), game);
    return map;
  }, [preview]);

  const selectable = useMemo(
    () => (preview ? preview.games.filter((game) => !game.imported) : []),
    [preview],
  );

  const needsUri = (game: LauncherGame) => Boolean(game.issue) && !uris[keyOf(game)]?.trim();

  const scan = async () => {
    setScanning(true);
    setError(null);
    try {
      const next = await native.launcherScan();
      setPreview(next);
      setSelected(new Set(next.games.filter((game) => !game.imported).map(keyOf)));
      setMapping({});
      setUris({});
      if (next.games.length === 0) {
        const detected = next.sources.filter((source) => source.available);
        push({
          title: t("No launcher games found"),
          description:
            next.warnings[0] ??
            (detected.length > 0
              ? t("{launchers} detected, but no installed game was listed.", {
                  launchers: detected.map((source) => source.label).join(", "),
                })
              : t("None of the supported launchers is installed on this machine.")),
        });
      }
    } catch (cause) {
      const message = errorMessage(cause);
      setError(message);
      push({ title: t("Could not read the launchers"), description: message, tone: "danger" });
    } finally {
      setScanning(false);
    }
  };

  const toggle = (key: string, checked: boolean) => {
    setSelected((current) => {
      const next = new Set(current);
      if (checked) next.add(key);
      else next.delete(key);
      return next;
    });
  };

  const importSelected = async () => {
    if (selected.size === 0) return;
    const entries: LauncherImportEntry[] = [];
    for (const key of selected) {
      const game = byKey.get(key);
      if (!game) continue;
      const manual = uris[key]?.trim();
      entries.push({
        source: game.source,
        externalId: game.externalId,
        gameId: mapping[key] ?? game.matchedGameId ?? null,
        launchUri: manual ? manual : null,
      });
    }
    if (entries.length === 0) return;
    setImporting(true);
    setError(null);
    try {
      const report = await native.launcherImport(entries);
      await queryClient.invalidateQueries({ queryKey: ["installed"] });
      await queryClient.invalidateQueries({ queryKey: ["recent-games"] });
      await queryClient.invalidateQueries({ queryKey: ["library"] });
      push({
        title: tn(
          report.imported.length,
          "1 launcher game imported",
          "{count} launcher games imported",
        ),
        description:
          report.skipped.length > 0
            ? t("{count} skipped: {reasons}", {
                count: report.skipped.length,
                reasons: report.skipped
                  .slice(0, 2)
                  .map((entry) => `${entry.title} (${entry.reason})`)
                  .join(", "),
              })
            : t("They launch through their own client and appear with the rest of the shelf."),
        tone: report.skipped.length > 0 ? "default" : "success",
      });
      onImported?.();
      await scan();
    } catch (cause) {
      const message = errorMessage(cause);
      setError(message);
      push({ title: t("Could not import the games"), description: message, tone: "danger" });
    } finally {
      setImporting(false);
    }
  };

  const allSelected = selectable.length > 0 && selected.size === selectable.length;
  const serverGames = titles.data?.items ?? [];

  return (
    <div>
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div>
          <p className="text-[13px] font-medium text-ink">{t("Launchers on this machine")}</p>
          <p className="mt-1 max-w-[56ch] text-[11.5px] leading-relaxed text-ink-3">
            {t(
              "Reads the metadata Steam, Epic, GOG, EA, Ubisoft, Xbox, itch, Heroic and Lutris already keep on disk. Selected games are added as entries that launch through their own client; nothing is downloaded, copied or moved, and uninstalling the Ludex entry never touches the game files.",
            )}
          </p>
        </div>
        <div className="flex items-center gap-2">
          {preview && selectable.length > 0 && (
            <Button
              variant="primary"
              size="sm"
              loading={importing}
              disabled={selected.size === 0}
              onClick={() => void importSelected()}
            >
              {tn(selected.size, "Import 1 game", "Import {count} games")}
            </Button>
          )}
          <Button
            variant="default"
            size="sm"
            icon={<RefreshCw className={cn("size-3.5", scanning && "animate-spin")} />}
            loading={scanning}
            onClick={() => void scan()}
          >
            {preview ? t("Rescan") : t("Scan launchers")}
          </Button>
        </div>
      </div>

      {error && (
        <p role="alert" className="mt-3 border-l-2 border-danger pl-3 text-[12.5px] text-danger">
          {error}
        </p>
      )}

      {preview && (
        <div className="mt-4 flex flex-col gap-3">
          <div className="flex flex-wrap items-center gap-x-2 gap-y-1.5">
            {preview.sources.map((source) => (
              <span
                key={source.id}
                title={source.detail}
                className={cn(
                  "inline-flex items-center gap-1.5 border px-2 py-[3px] font-mono text-[10px] uppercase tracking-[0.08em]",
                  source.available ? "border-line-2 text-ink-2" : "border-line text-ink-3/70",
                )}
              >
                {source.label}
                <span className={source.available ? "text-ink" : "text-ink-3"}>{source.found}</span>
              </span>
            ))}
          </div>

          <div className="flex flex-wrap items-baseline gap-x-4 gap-y-1 font-mono text-[10.5px] text-ink-3">
            <span>
              {tn(preview.games.length, "1 game", "{count} games")} ·{" "}
              {tn(preview.roots.length, "1 source read", "{count} sources read")}
            </span>
            {!preview.matched && (
              <span className="text-signal-text">{t("not matched to the server library")}</span>
            )}
            {preview.games.length > 0 && (
              <button
                type="button"
                className="underline-offset-4 hover:text-ink hover:underline"
                onClick={() =>
                  setSelected(allSelected ? new Set() : new Set(selectable.map(keyOf)))
                }
              >
                {allSelected ? t("Clear all") : t("Select all")}
              </button>
            )}
          </div>

          {preview.warnings.length > 0 && (
            <details className="text-[11.5px] text-ink-3">
              <summary className="cursor-pointer select-none">
                {tn(preview.warnings.length, "1 warning", "{count} warnings")}
              </summary>
              <ul className="mt-1.5 flex flex-col gap-1 border-l border-line pl-3">
                {preview.warnings.slice(0, 8).map((warning) => (
                  <li key={warning}>{warning}</li>
                ))}
              </ul>
            </details>
          )}

          {preview.games.length === 0 ? (
            <p className="flex items-start gap-2 border-t border-line pt-3 text-[12.5px] leading-relaxed text-ink-3">
              <Gamepad2 className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
              {t(
                "No installed launcher game was found. Start each launcher once so it writes its library metadata, then scan again. Steam can also be pointed at a specific folder in the desktop settings.",
              )}
            </p>
          ) : (
            <ul className={cn("overflow-y-auto border-t border-line", compact ? "max-h-[260px]" : "max-h-[420px]")}>
              {preview.games.map((game) => {
                const key = keyOf(game);
                return (
                  <LauncherRow
                    key={key}
                    game={game}
                    checked={game.imported || selected.has(key)}
                    needsUri={needsUri(game)}
                    uri={uris[key] ?? ""}
                    onUriChange={(value) => setUris((current) => ({ ...current, [key]: value }))}
                    onCheckedChange={(checked) => toggle(key, checked)}
                    mappedTitle={
                      mapping[key] !== undefined
                        ? serverGames.find((entry) => entry.id === mapping[key])?.title ?? null
                        : game.matchedTitle
                    }
                    manual={mapping[key] !== undefined}
                    onMap={(gameId) =>
                      setMapping((current) => {
                        const next = { ...current };
                        if (gameId === null) delete next[key];
                        else next[key] = gameId;
                        return next;
                      })
                    }
                    serverGames={serverGames}
                    titlesPending={titles.isPending}
                  />
                );
              })}
            </ul>
          )}
        </div>
      )}
    </div>
  );
}

function LauncherRow({
  game,
  checked,
  needsUri,
  uri,
  onUriChange,
  onCheckedChange,
  mappedTitle,
  manual,
  onMap,
  serverGames,
  titlesPending,
}: {
  game: LauncherGame;
  checked: boolean;
  needsUri: boolean;
  uri: string;
  onUriChange: (value: string) => void;
  onCheckedChange: (checked: boolean) => void;
  mappedTitle: string | null;
  manual: boolean;
  onMap: (gameId: number | null) => void;
  serverGames: GameSummary[];
  titlesPending: boolean;
}) {
  return (
    <li className="border-b border-line py-3">
      <div className="flex items-start gap-3">
        <span className="pt-0.5">
          <Checkbox
            checked={checked}
            onCheckedChange={onCheckedChange}
            label={t("Import {title}", { title: game.name })}
          />
        </span>
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
            <span className="truncate text-[13px] text-ink">{game.name}</span>
            <Badge tone="outline">{game.source}</Badge>
            {game.imported ? (
              <Badge tone="ok">{t("Imported")}</Badge>
            ) : needsUri ? (
              <Badge tone="danger">{t("Needs a launch URI")}</Badge>
            ) : mappedTitle ? (
              <Badge tone="signal">{manual ? t("Mapped") : t("Matched")}</Badge>
            ) : (
              <Badge tone="outline">{t("Not in the library")}</Badge>
            )}
            {game.sizeBytes > 0 && (
              <span className="num ml-auto text-[10.5px] text-ink-3">{formatBytes(game.sizeBytes)}</span>
            )}
          </div>

          <p className="mt-0.5 flex flex-wrap items-baseline gap-x-3 gap-y-0.5 font-mono text-[10.5px] text-ink-3">
            {game.detail && <span>{game.detail}</span>}
            {game.installDir && <span className="max-w-[52ch] truncate">{game.installDir}</span>}
          </p>

          {mappedTitle && mappedTitle !== game.name && (
            <p className="mt-0.5 text-[11px] text-ink-3">{t("Ludex: {title}", { title: mappedTitle })}</p>
          )}
          {game.issue && <p className="mt-1 text-[11.5px] leading-relaxed text-ink-3">{game.issue}</p>}

          {!game.imported && (
            <div className="mt-2 flex flex-wrap items-center gap-2">
              <GamePicker
                game={game}
                mappedTitle={mappedTitle}
                manual={manual}
                serverGames={serverGames}
                titlesPending={titlesPending}
                onMap={onMap}
              />
              {needsUri && (
                <Input
                  value={uri}
                  onChange={(event) => onUriChange(event.target.value)}
                  placeholder="origin://launchgame/Origin.OFR.50.0000557"
                  spellCheck={false}
                  aria-label={t("Launch URI for {title}", { title: game.name })}
                  className="h-8 min-w-[260px] flex-1 font-mono text-[11px]"
                />
              )}
            </div>
          )}
          {!game.imported && !needsUri && uri.trim() && (
            <p className="mt-1 break-all font-mono text-[10px] text-ink-3">
              {tr("uses {uri}", { uri: <span className="font-mono">{uri.trim()}</span> })}
            </p>
          )}
        </div>
      </div>
    </li>
  );
}

function GamePicker({
  game,
  mappedTitle,
  manual,
  serverGames,
  titlesPending,
  onMap,
}: {
  game: LauncherGame;
  mappedTitle: string | null;
  manual: boolean;
  serverGames: GameSummary[];
  titlesPending: boolean;
  onMap: (gameId: number | null) => void;
}) {
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState("");
  const ref = useRef<HTMLButtonElement>(null);

  const matches = useMemo(() => {
    const needle = search.trim().toLowerCase();
    const pool = needle
      ? serverGames.filter((entry) => entry.title.toLowerCase().includes(needle))
      : serverGames;
    return pool.slice(0, 12);
  }, [search, serverGames]);

  return (
    <>
      <button
        ref={ref}
        type="button"
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
        className="h-8 border border-line bg-surface px-2.5 text-[11px] text-ink-2 transition-colors hover:border-line-2 hover:text-ink"
      >
        {manual
          ? t("Mapped: {title}", { title: mappedTitle ?? "?" })
          : game.matchedGameId !== null
            ? t("Change mapping")
            : t("Map to a Ludex game")}
      </button>
      <Popover
        open={open}
        onOpenChange={setOpen}
        anchorRef={ref}
        align="start"
        label={t("Map {title}", { title: game.name })}
        className="w-[320px] p-2"
      >
        <div className="flex items-center gap-2 border border-line bg-surface px-2">
          <Search className="size-3.5 shrink-0 text-ink-3" aria-hidden="true" />
          <input
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            placeholder={t("Search the library")}
            aria-label={t("Search the library")}
            spellCheck={false}
            className="h-8 w-full bg-transparent text-[12px] text-ink outline-none placeholder:text-ink-3"
          />
        </div>
        <div className="mt-1.5 max-h-[240px] overflow-y-auto">
          {titlesPending ? (
            <p className="px-2 py-3 text-[11.5px] text-ink-3">{t("Loading the library…")}</p>
          ) : matches.length === 0 ? (
            <p className="px-2 py-3 text-[11.5px] text-ink-3">{t("No game matches that search.")}</p>
          ) : (
            matches.map((entry) => (
              <button
                key={entry.id}
                type="button"
                onClick={() => {
                  onMap(entry.id);
                  setOpen(false);
                }}
                className="flex w-full items-baseline justify-between gap-2 px-2 py-1.5 text-left text-[12px] text-ink-2 hover:bg-surface-2 hover:text-ink"
              >
                <span className="truncate">{entry.title}</span>
                <span className="num shrink-0 text-[10.5px] text-ink-3">
                  {entry.releaseDate ? entry.releaseDate.slice(0, 4) : ""}
                </span>
              </button>
            ))
          )}
        </div>
        {(manual || game.matchedGameId !== null) && (
          <button
            type="button"
            onClick={() => {
              onMap(null);
              setOpen(false);
            }}
            className="mt-1 w-full border-t border-line px-2 pt-1.5 text-left text-[11px] text-ink-3 hover:text-ink"
          >
            {t("Use the automatic match")}
          </button>
        )}
      </Popover>
    </>
  );
}
