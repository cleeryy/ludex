import { useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import {
  ArrowDown,
  ArrowUp,
  FolderOpen,
  FolderPlus,
  RefreshCw,
  ScanSearch,
  Trash2,
  Wand2,
} from "lucide-react";
import { isNative, native } from "@/lib/native";
import { installBridge } from "@/lib/install";
import { errorMessage } from "@/lib/http";
import { formatBytes } from "@/lib/format";
import { useInstalledGames } from "@/lib/hooks";
import type {
  GameVaultCandidate,
  GameVaultRootProposal,
  GameVaultScan,
} from "@/lib/types";
import { useToast } from "@/state/toasts";
import { Badge, Button, Checkbox, Field, Input } from "@/components/ui/controls";
import { Select } from "@/components/ui/select";
import { AdoptInstallDialog } from "@/components/game/adopt-install";
import { t, tn } from "@/lib/i18n";

/** Comparison key for a folder path in the UI only. */
function pathKey(value: string): string {
  return value.replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();
}

/** Source chip for an installed row. */
function SourceBadge({ source }: { source?: string }) {
  if (source === "adopted") return <Badge tone="signal">{t("Adopted")}</Badge>;
  if (source === "ludex" || !source) return <Badge tone="outline">{t("Ludex")}</Badge>;
  return <Badge tone="outline">{source}</Badge>;
}

/**
 * Settings → Installed (#129, #134): what this machine has, with the source,
 * plus the GameVault folder scan. Adoption links folders in place: GameVault
 * metadata is only read, never written, and nothing is copied or moved.
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
                    {entry.gameId > 0 ? (
                      <Link to={`/game/${entry.gameId}`} className="truncate text-[13px] text-ink hover:underline">
                        {entry.title}
                      </Link>
                    ) : (
                      <span className="truncate text-[13px] text-ink">{entry.title}</span>
                    )}
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
  const settingsQuery = useQuery({
    queryKey: ["desktop-settings"],
    queryFn: () => installBridge.settings(),
    enabled: isNative(),
  });
  const settings = settingsQuery.data ?? null;
  const roots = settings?.gamevaultRoots ?? [];
  const depth = settings?.gamevaultScanDepth ?? 6;

  const [newRoot, setNewRoot] = useState("");
  const [scan, setScan] = useState<GameVaultScan | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [scanning, setScanning] = useState(false);
  const [adopting, setAdopting] = useState(false);
  const [detecting, setDetecting] = useState(false);
  const [proposals, setProposals] = useState<GameVaultRootProposal[]>([]);
  const [linkCandidate, setLinkCandidate] = useState<GameVaultCandidate | null>(null);
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

  const refreshSettings = async () => {
    await queryClient.invalidateQueries({ queryKey: ["desktop-settings"] });
  };

  const resetScan = () => {
    setScan(null);
    setSelected(new Set());
  };

  const addRoot = async (path: string) => {
    const value = path.trim();
    if (!value) return;
    setError(null);
    try {
      await native.addGameVaultRoot(value);
      setNewRoot("");
      setProposals([]);
      resetScan();
      await refreshSettings();
      push({ title: t("GameVault folder added"), description: value, tone: "success" });
    } catch (cause) {
      const message = errorMessage(cause);
      setError(message);
      push({ title: t("Could not add the folder"), description: message, tone: "danger" });
    }
  };

  const browse = async () => {
    try {
      const picked = await native.pickFolder(t("Choose a GameVault install folder"));
      if (picked) await addRoot(picked);
    } catch (cause) {
      push({ title: t("Could not open the folder picker"), description: errorMessage(cause), tone: "danger" });
    }
  };

  const removeRoot = async (root: string) => {
    setError(null);
    try {
      await native.removeGameVaultRoot(root);
      resetScan();
      await refreshSettings();
    } catch (cause) {
      const message = errorMessage(cause);
      setError(message);
      push({ title: t("Could not remove the folder"), description: message, tone: "danger" });
    }
  };

  const moveRoot = async (index: number, delta: number) => {
    const target = index + delta;
    if (target < 0 || target >= roots.length) return;
    const next = [...roots];
    const [entry] = next.splice(index, 1);
    next.splice(target, 0, entry);
    setError(null);
    try {
      await native.reorderGameVaultRoots(next);
      await refreshSettings();
    } catch (cause) {
      const message = errorMessage(cause);
      setError(message);
      push({ title: t("Could not reorder the folders"), description: message, tone: "danger" });
    }
  };

  const changeDepth = async (value: number) => {
    if (!settings) return;
    setError(null);
    try {
      await installBridge.saveSettings({ ...settings, gamevaultScanDepth: value });
      await refreshSettings();
      resetScan();
    } catch (cause) {
      const message = errorMessage(cause);
      setError(message);
      push({ title: t("Could not save the scan depth"), description: message, tone: "danger" });
    }
  };

  const detect = async () => {
    setDetecting(true);
    setError(null);
    try {
      const found = await native.detectGameVaultRoots();
      setProposals(found.filter((proposal) => !proposal.configured));
    } catch (cause) {
      const message = errorMessage(cause);
      setError(message);
      push({ title: t("Could not detect GameVault folders"), description: message, tone: "danger" });
    } finally {
      setDetecting(false);
    }
  };

  const runScan = async () => {
    if (roots.length === 0) return;
    setScanning(true);
    setError(null);
    try {
      const next = await native.scanGameVaultFolders(roots);
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
      push({ title: t("Could not scan the folders"), description: message, tone: "danger" });
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
        executable: candidate.executable,
      }));
    if (entries.length === 0) return;
    const scannedRoots = scan.roots.filter((root) => !root.error).map((root) => root.root);
    setAdopting(true);
    setError(null);
    try {
      const report = await native.adoptGameVaultInstallations(scannedRoots, entries);
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

  const rootStatus = (root: string) => {
    if (!scan) return null;
    return scan.roots.find((entry) => pathKey(entry.root) === pathKey(root)) ?? null;
  };

  return (
    <div>
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div>
          <p className="text-[13px] font-medium text-ink">{t("GameVault folders")}</p>
          <p className="mt-1 max-w-[56ch] text-[11.5px] leading-relaxed text-ink-3">
            {t("Add every folder that contains one subfolder per game, for example C:\\Games\\GameVault and D:\\Games\\GameVault. Ludex scans them together, matches folders to your library and links the ones you select. GameVault metadata is only read; nothing is written, copied or moved.")}
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
            disabled={roots.length === 0}
            onClick={() => void runScan()}
          >
            {scan ? t("Rescan all") : t("Scan all")}
          </Button>
        </div>
      </div>

      <div className="mt-3 border-t border-line pt-4">
        <div className="flex flex-wrap items-end gap-2">
          <Field label={t("GameVault folder")} htmlFor="gamevault-root" className="min-w-[260px] flex-1">
            <Input
              id="gamevault-root"
              value={newRoot}
              onChange={(event) => setNewRoot(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  void addRoot(newRoot);
                }
              }}
              placeholder="D:\\Games\\GameVault"
              spellCheck={false}
              disabled={!isNative()}
              className="h-10 font-mono text-[11.5px]"
            />
          </Field>
          <Button
            variant="primary"
            size="sm"
            className="mb-[1px] h-10 shrink-0"
            icon={<FolderPlus className="size-3.5" />}
            onClick={() => void addRoot(newRoot)}
            disabled={!isNative() || !newRoot.trim()}
          >
            {t("Add folder")}
          </Button>
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
          <Button
            variant="ghost"
            size="sm"
            className="mb-[1px] h-10 shrink-0"
            icon={<Wand2 className="size-3.5" />}
            loading={detecting}
            onClick={() => void detect()}
            disabled={!isNative()}
          >
            {t("Detect")}
          </Button>
        </div>

        {roots.length > 0 && (
          <ul className="mt-3 border-t border-line">
            {roots.map((root, index) => {
              const status = rootStatus(root);
              return (
                <li key={root} className="flex flex-wrap items-center gap-3 border-b border-line py-2.5">
                  <span className="num w-5 shrink-0 text-[11px] text-ink-3">{index + 1}</span>
                  <div className="min-w-0 flex-1">
                    <p className="truncate font-mono text-[11.5px] text-ink-2" title={root}>
                      {root}
                    </p>
                    <p className="mt-0.5 flex flex-wrap items-baseline gap-x-3 gap-y-0.5 text-[11px] text-ink-3">
                      {status?.error ? (
                        <span className="text-danger">{status.error}</span>
                      ) : status ? (
                        <span>{tn(status.candidates, "1 folder found", "{count} folders found")}</span>
                      ) : (
                        <span>{t("Not scanned yet")}</span>
                      )}
                      {status && status.warnings.length > 0 && (
                        <span>
                          {tn(status.warnings.length, "1 warning", "{count} warnings")}
                        </span>
                      )}
                    </p>
                  </div>
                  <div className="flex shrink-0 items-center gap-1">
                    <Button
                      variant="ghost"
                      size="sm"
                      aria-label={t("Move up")}
                      title={t("Move up")}
                      disabled={index === 0}
                      onClick={() => void moveRoot(index, -1)}
                    >
                      <ArrowUp className="size-3.5" aria-hidden="true" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="sm"
                      aria-label={t("Move down")}
                      title={t("Move down")}
                      disabled={index === roots.length - 1}
                      onClick={() => void moveRoot(index, 1)}
                    >
                      <ArrowDown className="size-3.5" aria-hidden="true" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="sm"
                      aria-label={t("Remove {folder}", { folder: root })}
                      title={t("Remove")}
                      onClick={() => void removeRoot(root)}
                    >
                      <Trash2 className="size-3.5" aria-hidden="true" />
                    </Button>
                  </div>
                </li>
              );
            })}
          </ul>
        )}

        <div className="mt-3 flex flex-wrap items-end gap-3">
          <Field
            label={t("Search depth")}
            htmlFor="gamevault-depth"
            hint={t("Folder levels searched inside each game folder. 4-8, default 6.")}
          >
            <div className="w-[110px]">
              <Select<number>
                id="gamevault-depth"
                label={t("Search depth")}
                value={depth}
                onChange={(value) => void changeDepth(value)}
                options={[4, 5, 6, 7, 8].map((value) => ({ value, label: String(value) }))}
              />
            </div>
          </Field>
        </div>

        {proposals.length > 0 && (
          <div className="mt-4 border border-line bg-surface/50 p-3">
            <p className="text-[12.5px] text-ink">{t("GameVault folders found on this machine")}</p>
            <p className="mt-0.5 text-[11.5px] leading-relaxed text-ink-3">
              {t("Detection is read-only: Ludex only looked at the usual locations and GameVault's own config files. Add the ones you want; nothing was changed.")}
            </p>
            <ul className="mt-2 flex flex-col gap-2">
              {proposals.map((proposal) => (
                <li key={proposal.path} className="flex flex-wrap items-center gap-3">
                  <div className="min-w-0 flex-1">
                    <p className="truncate font-mono text-[11.5px] text-ink-2" title={proposal.path}>
                      {proposal.path}
                    </p>
                    <p className="mt-0.5 text-[11px] text-ink-3">
                      {proposal.reason}{" "}
                      {proposal.gameFolders > 0 &&
                        tn(proposal.gameFolders, "1 game folder", "{count} game folders")}
                    </p>
                  </div>
                  <Button
                    variant="default"
                    size="sm"
                    disabled={proposal.configured}
                    onClick={() => void addRoot(proposal.path)}
                  >
                    {proposal.configured ? t("Already added") : t("Add")}
                  </Button>
                </li>
              ))}
            </ul>
          </div>
        )}
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
            <span>
              {tn(scan.candidates.length, "1 folder", "{count} folders")} ·{" "}
              {tn(scan.candidates.length - selectable.length, "1 already linked or unmatched", "{count} already linked or unmatched")}
            </span>
            <span>{t("depth {depth}", { depth: scan.maxDepth })}</span>
            {!scan.matched && <span className="text-signal-text">{t("not matched to the server library")}</span>}
          </div>

          {scan.warnings.length > 0 && (
            <details className="text-[11.5px] text-ink-3">
              <summary className="cursor-pointer select-none">
                {tn(scan.warnings.length, "1 warning", "{count} warnings")}
              </summary>
              <ul className="mt-1.5 flex flex-col gap-1 border-l border-line pl-3">
                {scan.warnings.slice(0, 12).map((warning) => (
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
            <ul className="max-h-[480px] overflow-y-auto border-t border-line">
              {scan.candidates.map((candidate) => {
                const status = statusLabel(candidate);
                const selectableRow =
                  candidate.matchedGameId !== null && candidate.installed === null;
                return (
                  <li key={candidate.folder} className="flex items-start gap-3 border-b border-line py-3">
                    <span className="pt-0.5">
                      {selectableRow ? (
                        <Checkbox
                          checked={selected.has(candidate.folder)}
                          onCheckedChange={(checked) => toggle(candidate.folder, checked)}
                          label={t("Link {name}", { name: candidate.name })}
                        />
                      ) : (
                        <span className="block size-5" aria-hidden="true" />
                      )}
                    </span>
                    <div className="min-w-0 flex-1">
                      <div className="flex flex-wrap items-baseline gap-x-2 gap-y-1">
                        <span className="truncate text-[13px] text-ink">{candidate.name}</span>
                        <Badge tone={status.tone}>{status.label}</Badge>
                        <Badge tone={candidate.executable ? "outline" : "signal"}>
                          {candidate.executable ? t("Executable found") : t("No executable")}
                        </Badge>
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
                      <p className="mt-0.5 truncate text-[10.5px] text-ink-3" title={candidate.sourceRoot}>
                        {t("From {root}", { root: candidate.sourceRoot })}
                      </p>
                      {candidate.executable && (
                        <p className="mt-0.5 truncate font-mono text-[10px] text-ink-3" title={candidate.executable}>
                          {candidate.executable}
                        </p>
                      )}
                      {candidate.executableCandidates.length > 1 && (
                        <details className="mt-1 text-[11px] text-ink-3">
                          <summary className="cursor-pointer select-none">
                            {tn(
                              candidate.executableCandidates.length,
                              "1 executable found",
                              "{count} executables found",
                            )}
                          </summary>
                          <ul className="mt-1 flex flex-col gap-0.5 border-l border-line pl-3 font-mono text-[10px]">
                            {candidate.executableCandidates.slice(0, 6).map((path) => (
                              <li key={path} className="truncate" title={path}>
                                {path}
                              </li>
                            ))}
                          </ul>
                        </details>
                      )}
                      {candidate.notes.map((note) => (
                        <p key={note} className="mt-1 text-[11.5px] leading-relaxed text-ink-3">
                          {note}
                        </p>
                      ))}
                      {candidate.warnings.map((warning) => (
                        <p key={warning} className="mt-1 text-[11.5px] leading-relaxed text-danger">
                          {warning}
                        </p>
                      ))}
                      {selectableRow && (
                        <Button
                          variant="ghost"
                          size="sm"
                          className="mt-1.5 h-7 px-2"
                          onClick={() => setLinkCandidate(candidate)}
                        >
                          {candidate.executable ? t("Choose the executable") : t("Link anyway")}
                        </Button>
                      )}
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

      <AdoptInstallDialog
        open={linkCandidate !== null}
        onOpenChange={(next) => {
          if (!next) setLinkCandidate(null);
        }}
        gameId={linkCandidate?.matchedGameId ?? 0}
        title={linkCandidate?.matchedTitle ?? linkCandidate?.name ?? ""}
        versions={[]}
        initialFolder={linkCandidate?.folder}
        initialExecutable={linkCandidate?.executable}
        onAdopted={() => {
          setLinkCandidate(null);
          void runScan();
        }}
      />
    </div>
  );
}
