import { useMemo, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import { FolderOpen, RefreshCw, ScanSearch } from "lucide-react";
import { isNative, native } from "@/lib/native";
import { errorMessage } from "@/lib/http";
import { formatBytes } from "@/lib/format";
import { useInstalledGames } from "@/lib/hooks";
import type { GameVaultCandidate, GameVaultScan } from "@/lib/types";
import { useToast } from "@/state/toasts";
import { Badge, Button, Checkbox, Field, Input } from "@/components/ui/controls";
import { t, tn } from "@/lib/i18n";

/** Source chip for an installed row. */
function SourceBadge({ source }: { source?: string }) {
  if (source === "adopted") return <Badge tone="signal">{t("Adopted")}</Badge>;
  if (source === "ludex" || !source) return <Badge tone="outline">{t("Ludex")}</Badge>;
  return <Badge tone="outline">{source}</Badge>;
}

/**
 * Settings → Installed (#129): what this machine has, with the source, plus the
 * GameVault folder scan. Adoption links folders in place: GameVault metadata is
 * only read, never written, and nothing is copied or moved.
 */
export function InstalledPanel() {
  return (
    <div className="flex flex-col gap-10">
      <InstalledList />
      <GameVaultImportPanel />
    </div>
  );
}

function InstalledList() {
  const installed = useInstalledGames();
  const games = installed.data ?? [];

  return (
    <div>
      <p className="text-[13px] font-medium text-ink">{t("On this device")}</p>
      <p className="mt-1 max-w-[56ch] text-[11.5px] leading-relaxed text-ink-3">
        {t("Installs made by Ludex, folders you linked, and games imported from another launcher. Adopted folders keep their files where they are.")}
      </p>
      <div className="mt-3 border-t border-line">
        {games.length === 0 ? (
          <p className="py-3 text-[12.5px] text-ink-3">{t("Nothing is installed on this machine yet.")}</p>
        ) : (
          <ul>
            {games.map((entry) => (
              <li key={entry.gameId} className="flex items-center gap-3 border-b border-line py-2.5">
                <div className="min-w-0 flex-1">
                  <div className="flex flex-wrap items-baseline gap-x-2 gap-y-1">
                    <Link to={`/game/${entry.gameId}`} className="truncate text-[13px] text-ink hover:underline">
                      {entry.title}
                    </Link>
                    <SourceBadge source={entry.source} />
                    {entry.version && <span className="font-mono text-[10.5px] text-ink-3">v{entry.version}</span>}
                  </div>
                  <p className="mt-0.5 flex flex-wrap items-baseline gap-x-3 font-mono text-[10.5px] text-ink-3">
                    <span className="num">{formatBytes(entry.sizeBytes)}</span>
                    <span className="max-w-[60ch] truncate" title={entry.installPath}>
                      {entry.installPath || t("No folder")}
                    </span>
                  </p>
                </div>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

function GameVaultImportPanel() {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const [root, setRoot] = useState("");
  const [scan, setScan] = useState<GameVaultScan | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [scanning, setScanning] = useState(false);
  const [adopting, setAdopting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const selectable = useMemo(
    () =>
      (scan?.candidates ?? []).filter(
        (candidate) => candidate.matchedGameId !== null && candidate.installed === null,
      ),
    [scan],
  );

  const statusLabel = (candidate: GameVaultCandidate) => {
    if (candidate.installed) return { tone: "outline" as const, label: t("Already in Ludex") };
    switch (candidate.matchStatus) {
      case "exact":
        return { tone: "ok" as const, label: t("Matched") };
      case "containment":
        return { tone: "signal" as const, label: t("Approximate match") };
      case "ambiguous":
        return { tone: "danger" as const, label: t("Ambiguous") };
      default:
        return { tone: "outline" as const, label: t("Not in the library") };
    }
  };

  const browse = async () => {
    try {
      const picked = await native.pickFolder(t("Choose the GameVault install folder"));
      if (picked) {
        setRoot(picked);
        setScan(null);
        setSelected(new Set());
      }
    } catch (cause) {
      push({ title: t("Could not open the folder picker"), description: errorMessage(cause), tone: "danger" });
    }
  };

  const runScan = async () => {
    if (!root.trim()) return;
    setScanning(true);
    setError(null);
    try {
      const next = await native.scanGameVaultFolders(root.trim());
      setScan(next);
      setSelected(
        new Set(
          next.candidates
            .filter((candidate) => candidate.matchedGameId !== null && candidate.installed === null)
            .map((candidate) => candidate.folder),
        ),
      );
      if (next.candidates.length === 0) {
        push({
          title: t("No game folder found"),
          description:
            next.warnings[0] ??
            t("Each GameVault game lives in its own folder with the game executable inside."),
        });
      }
    } catch (cause) {
      const message = errorMessage(cause);
      setError(message);
      push({ title: t("Could not scan the folder"), description: message, tone: "danger" });
    } finally {
      setScanning(false);
    }
  };

  const toggle = (folder: string, checked: boolean) => {
    setSelected((current) => {
      const next = new Set(current);
      if (checked) next.add(folder);
      else next.delete(folder);
      return next;
    });
  };

  const adoptSelected = async () => {
    if (!scan || selected.size === 0) return;
    const entries = selectable
      .filter((candidate) => selected.has(candidate.folder))
      .map((candidate) => ({
        folder: candidate.folder,
        gameId: candidate.matchedGameId as number,
        versionId: 0,
        version: null,
      }));
    if (entries.length === 0) return;
    setAdopting(true);
    setError(null);
    try {
      const report = await native.adoptGameVaultInstallations(scan.root, entries);
      await queryClient.invalidateQueries({ queryKey: ["installed"] });
      await queryClient.invalidateQueries({ queryKey: ["recent-games"] });
      await queryClient.invalidateQueries({ queryKey: ["games"] });
      push({
        title: tn(report.adopted.length, "1 folder linked", "{count} folders linked"),
        description:
          report.skipped.length > 0
            ? t("{count} skipped: {reasons}", {
                count: report.skipped.length,
                reasons: report.skipped
                  .slice(0, 2)
                  .map((entry) => `${entry.folder} (${entry.reason})`)
                  .join(", "),
              })
            : t("They launch from their own folder. Nothing was copied or moved."),
        tone: report.skipped.length > 0 ? "default" : "success",
      });
      await runScan();
    } catch (cause) {
      const message = errorMessage(cause);
      setError(message);
      push({ title: t("Could not link the folders"), description: message, tone: "danger" });
    } finally {
      setAdopting(false);
    }
  };

  return (
    <div>
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div>
          <p className="text-[13px] font-medium text-ink">{t("GameVault folders")}</p>
          <p className="mt-1 max-w-[56ch] text-[11.5px] leading-relaxed text-ink-3">
            {t("Scan a folder that contains one subfolder per game. Ludex lists the folders with a game executable, matches them to your library and links the ones you select. GameVault metadata is only read; nothing is written, copied or moved.")}
          </p>
        </div>
        <div className="flex items-center gap-2">
          {scan && selectable.length > 0 && (
            <Button
              variant="primary"
              size="sm"
              loading={adopting}
              disabled={selected.size === 0}
              onClick={() => void adoptSelected()}
            >
              {tn(selected.size, "Link 1 folder", "Link {count} folders")}
            </Button>
          )}
          <Button
            variant="default"
            size="sm"
            icon={<RefreshCw className={scanning ? "size-3.5 animate-spin" : "size-3.5"} />}
            loading={scanning}
            onClick={() => void runScan()}
          >
            {scan ? t("Rescan") : t("Scan folder")}
          </Button>
        </div>
      </div>

      <div className="mt-3 flex items-end gap-2 border-t border-line pt-4">
        <Field label={t("GameVault folder")} htmlFor="gamevault-root" className="flex-1">
          <Input
            id="gamevault-root"
            value={root}
            onChange={(event) => {
              setRoot(event.target.value);
              setScan(null);
              setSelected(new Set());
            }}
            placeholder="/mnt/games/GameVault"
            spellCheck={false}
            disabled={!isNative()}
            className="h-10 font-mono text-[11.5px]"
          />
        </Field>
        <Button
          variant="default"
          size="sm"
          className="mb-[1px] h-10 shrink-0"
          icon={<FolderOpen className="size-3.5" />}
          onClick={() => void browse()}
          disabled={!isNative()}
        >
          {t("Browse")}
        </Button>
      </div>

      {!isNative() && (
        <p className="mt-2 text-[11.5px] text-ink-3">{t("Linking a folder needs the desktop app.")}</p>
      )}

      {error && (
        <p role="alert" className="mt-3 border-l-2 border-danger pl-3 text-[12.5px] text-danger">
          {error}
        </p>
      )}

      {scan && (
        <div className="mt-4 flex flex-col gap-3">
          <div className="flex flex-wrap items-baseline gap-x-4 gap-y-1 font-mono text-[10.5px] text-ink-3">
            <span className="truncate" title={scan.root}>
              {scan.root}
            </span>
            <span>
              {tn(scan.candidates.length, "1 folder", "{count} folders")} ·{" "}
              {tn(scan.candidates.length - selectable.length, "1 already linked or unmatched", "{count} already linked or unmatched")}
            </span>
            {!scan.matched && <span className="text-signal-text">{t("not matched to the server library")}</span>}
          </div>

          {scan.warnings.length > 0 && (
            <details className="text-[11.5px] text-ink-3">
              <summary className="cursor-pointer select-none">
                {tn(scan.warnings.length, "1 warning", "{count} warnings")}
              </summary>
              <ul className="mt-1.5 flex flex-col gap-1 border-l border-line pl-3">
                {scan.warnings.slice(0, 8).map((warning) => (
                  <li key={warning}>{warning}</li>
                ))}
              </ul>
            </details>
          )}

          {scan.candidates.length === 0 ? (
            <p className="flex items-start gap-2 border-t border-line pt-3 text-[12.5px] leading-relaxed text-ink-3">
              <ScanSearch className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
              {t("No subfolder here contains a game executable. Pick the folder that holds the downloaded games, not the app's own folder.")}
            </p>
          ) : (
            <ul className="max-h-[420px] overflow-y-auto border-t border-line">
              {scan.candidates.map((candidate) => {
                const status = statusLabel(candidate);
                const selectableRow =
                  candidate.matchedGameId !== null && candidate.installed === null;
                return (
                  <li key={candidate.folder} className="flex items-start gap-3 border-b border-line py-3">
                    <span className="pt-0.5">
                      <Checkbox
                        checked={selected.has(candidate.folder)}
                        onCheckedChange={(checked) => toggle(candidate.folder, checked)}
                        label={t("Link {name}", { name: candidate.name })}
                      />
                    </span>
                    <div className="min-w-0 flex-1">
                      <div className="flex flex-wrap items-baseline gap-x-2 gap-y-1">
                        <span className="truncate text-[13px] text-ink">{candidate.name}</span>
                        <Badge tone={status.tone}>{status.label}</Badge>
                        {candidate.matchedTitle && (
                          <span className="text-[11px] text-ink-3">
                            {t("Ludex: {title}", { title: candidate.matchedTitle })}
                          </span>
                        )}
                        <span className="num ml-auto text-[10.5px] text-ink-3">
                          {formatBytes(candidate.sizeBytes)} ·{" "}
                          {tn(candidate.fileCount, "1 file", "{count} files")}
                        </span>
                      </div>
                      <p className="mt-0.5 truncate font-mono text-[10.5px] text-ink-3" title={candidate.folder}>
                        {candidate.folder}
                      </p>
                      {candidate.executable && (
                        <p className="mt-0.5 truncate font-mono text-[10px] text-ink-3" title={candidate.executable}>
                          {candidate.executable}
                        </p>
                      )}
                      {candidate.notes.map((note) => (
                        <p key={note} className="mt-1 text-[11.5px] leading-relaxed text-ink-3">
                          {note}
                        </p>
                      ))}
                      {!selectableRow && candidate.matchStatus === "ambiguous" && (
                        <p className="mt-1 text-[11.5px] leading-relaxed text-signal-text">
                          {t("Open the game page and link this folder there to choose the game and version yourself.")}
                        </p>
                      )}
                    </div>
                  </li>
                );
              })}
            </ul>
          )}
        </div>
      )}
    </div>
  );
}
