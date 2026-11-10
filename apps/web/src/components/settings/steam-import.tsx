import { useMemo, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Gamepad2, RefreshCw } from "lucide-react";
import { native } from "@/lib/native";
import { errorMessage } from "@/lib/http";
import { formatBytes } from "@/lib/format";
import type { SteamGame, SteamPreview } from "@/lib/types";
import { useToast } from "@/state/toasts";
import { Badge, Button, Checkbox } from "@/components/ui/controls";
import { t , tn } from "@/lib/i18n";

/**
 * Steam library import (#13). Ludex never downloads Steam games: the selected
 * entries are recorded as installed and launch through the Steam client.
 */
export function SteamImportPanel() {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const [preview, setPreview] = useState<SteamPreview | null>(null);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [scanning, setScanning] = useState(false);
  const [importing, setImporting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const selectable = useMemo(
    () => (preview ? preview.games.filter((game) => !game.imported) : []),
    [preview],
  );

  const scan = async () => {
    setScanning(true);
    setError(null);
    try {
      const next = await native.steamScan();
      setPreview(next);
      setSelected(new Set(next.games.filter((game) => !game.imported).map((game) => game.appId)));
      if (next.games.length === 0) {
        push({
          title: t("No Steam games found"),
          description:
            next.warnings[0] ?? "Steam is not installed here, or its library has no installed games.",
        });
      }
    } catch (cause) {
      const message = errorMessage(cause);
      setError(message);
      push({ title: t("Could not read the Steam library"), description: message, tone: "danger" });
    } finally {
      setScanning(false);
    }
  };

  const toggle = (appId: number, checked: boolean) => {
    setSelected((current) => {
      const next = new Set(current);
      if (checked) next.add(appId);
      else next.delete(appId);
      return next;
    });
  };

  const importSelected = async () => {
    const appIds = [...selected];
    if (appIds.length === 0) return;
    setImporting(true);
    setError(null);
    try {
      const report = await native.steamImport(appIds);
      await queryClient.invalidateQueries({ queryKey: ["installed"] });
      await queryClient.invalidateQueries({ queryKey: ["recent-games"] });
      push({
        title:
          report.imported.length === 1
            ? "1 Steam game imported"
            : `${report.imported.length} Steam games imported`,
        description:
          report.skipped.length > 0
            ? `${report.skipped.length} skipped: ${report.skipped
                .slice(0, 2)
                .map((entry) => `${entry.title} (${entry.reason})`)
                .join(", ")}`
            : "They launch through Steam and appear with the rest of your library.",
        tone: report.skipped.length > 0 ? "default" : "success",
      });
      await scan();
    } catch (cause) {
      const message = errorMessage(cause);
      setError(message);
      push({ title: t("Could not import the Steam games"), description: message, tone: "danger" });
    } finally {
      setImporting(false);
    }
  };

  return (
    <div>
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div>
          <p className="text-[13px] font-medium text-ink">{t("Steam library")}</p>
          <p className="mt-1 max-w-[52ch] text-[11.5px] leading-relaxed text-ink-3">
            Reads Steam&apos;s own manifests on this machine and adds the games you select to Ludex. Nothing is
            downloaded: they launch through the Steam client, and Steam keeps updates and DRM.
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
              {t("Import")} {selected.size === 1 ? t("1 game") : `${selected.size} games`}
            </Button>
          )}
          <Button
            variant="default"
            size="sm"
            icon={<RefreshCw className={scanning ? "size-3.5 animate-spin" : "size-3.5"} />}
            loading={scanning}
            onClick={() => void scan()}
          >
            {preview ? t("Rescan") : t("Import from Steam")}
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
          <div className="flex flex-wrap items-baseline gap-x-4 gap-y-1 font-mono text-[10.5px] text-ink-3">
            <span>
              {preview.games.length} {preview.games.length === 1 ? "game" : "games"} ·{" "}
              {preview.roots.length} {preview.roots.length === 1 ? "library" : "libraries"}
            </span>
            {!preview.matched && <span className="text-signal-text">{t("not matched to the server library")}</span>}
            {preview.games.length > 0 && (
              <button
                type="button"
                className="underline-offset-4 hover:text-ink hover:underline"
                onClick={() =>
                  setSelected(
                    selected.size === selectable.length
                      ? new Set()
                      : new Set(selectable.map((game) => game.appId)),
                  )
                }
              >
                {selected.size === selectable.length && selectable.length > 0 ? t("Clear all") : t("Select all")}
              </button>
            )}
          </div>

          {preview.warnings.length > 0 && (
            <p className="text-[11.5px] text-ink-3">
              {tn(preview.warnings.length, "1 warning", "{count} warnings")}: {preview.warnings[0]}
            </p>
          )}

          {preview.games.length === 0 ? (
            <p className="flex items-start gap-2 border-t border-line pt-3 text-[12.5px] leading-relaxed text-ink-3">
              <Gamepad2 className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
              {t("No installed Steam game was found. Start Steam once so it writes its library file, or set a Steam folder in the desktop settings.")}
            </p>
          ) : (
            <ul className="max-h-[340px] overflow-y-auto border-t border-line">
              {preview.games.map((game) => (
                <SteamRow
                  key={game.appId}
                  game={game}
                  checked={selected.has(game.appId)}
                  onCheckedChange={(checked) => toggle(game.appId, checked)}
                />
              ))}
            </ul>
          )}
        </div>
      )}
    </div>
  );
}

function SteamRow({
  game,
  checked,
  onCheckedChange,
}: {
  game: SteamGame;
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
}) {
  return (
    <li className="flex items-start gap-3 border-b border-line py-2.5">
      <span className="pt-0.5">
        <Checkbox
          checked={game.imported || checked}
          onCheckedChange={onCheckedChange}
          label={t("Import {name}", { name: game.name })}
        />
      </span>
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-baseline gap-x-3 gap-y-0.5">
          <span className="truncate text-[13px] text-ink">{game.name}</span>
          {game.imported ? (
            <Badge tone="ok">{t("Imported")}</Badge>
          ) : game.matchedGameId !== null ? (
            <Badge tone="signal">
              {t("Matched")}
            </Badge>
          ) : (
            <Badge tone="outline">{t("Not in the library")}</Badge>
          )}
          <span className="num ml-auto text-[10.5px] text-ink-3">{formatBytes(game.sizeBytes)}</span>
        </div>
        <p className="mt-0.5 flex flex-wrap items-baseline gap-x-3 font-mono text-[10.5px] text-ink-3">
          <span>{t("App {id}", { id: game.appId })}</span>
          {game.matchedTitle && game.matchedTitle !== game.name && <span>{t("Ludex:")} {game.matchedTitle}</span>}
          {game.matchedGameId === null && <span>{t("local id")} {game.localGameId}</span>}
        </p>
      </div>
    </li>
  );
}
