import { useEffect, useMemo, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { FolderOpen, HardDrive, Link2 } from "lucide-react";
import { isNative, native } from "@/lib/native";
import { errorMessage } from "@/lib/http";
import { formatBytes } from "@/lib/format";
import type { AdoptPreview } from "@/lib/types";
import { useToast } from "@/state/toasts";
import { Badge, Button, Checkbox, Field, Input } from "@/components/ui/controls";
import { Dialog } from "@/components/ui/dialog";
import { Select } from "@/components/ui/select";
import { t, tn } from "@/lib/i18n";

/** Path relative to the adopted folder, for the candidate picker. */
function relativeToFolder(path: string, folder: string): string {
  const normalise = (value: string) => value.replace(/\\/g, "/").replace(/\/+$/, "");
  const base = normalise(folder);
  const full = path.replace(/\\/g, "/");
  return full.startsWith(`${base}/`) ? full.slice(base.length + 1) : path;
}

/**
 * "Link existing installation" (#129).
 *
 * Points Ludex at a folder that is already on disk — typically a GameVault
 * download. The folder is inspected first (executable, size, file count,
 * version) and nothing is written until the user confirms. No file is copied
 * or moved.
 */
export function AdoptInstallDialog({
  open,
  onOpenChange,
  gameId,
  title,
  versions,
  initialFolder,
  initialExecutable,
  onAdopted,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  gameId: number;
  title: string;
  versions: { id: number; version: string | null; fileName: string }[];
  /** Prefill from a GameVault candidate (#134). */
  initialFolder?: string;
  /** Manual executable ("link anyway") preselected for this folder (#134). */
  initialExecutable?: string | null;
  onAdopted?: () => void;
}) {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const [folder, setFolder] = useState("");
  const [executable, setExecutable] = useState("");
  const [versionId, setVersionId] = useState(0);
  const [preview, setPreview] = useState<AdoptPreview | null>(null);
  const [replace, setReplace] = useState(false);
  const [busy, setBusy] = useState<"check" | "adopt" | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (open) {
      setFolder(initialFolder ?? "");
      setExecutable(initialExecutable ?? "");
      setVersionId(0);
      setPreview(null);
      setReplace(false);
      setBusy(null);
      setError(null);
    } else {
      setFolder("");
      setExecutable("");
      setVersionId(0);
      setPreview(null);
      setReplace(false);
      setBusy(null);
      setError(null);
    }
  }, [open, initialFolder, initialExecutable]);

  // Any change invalidates the inspection: the confirmation is about one folder.
  useEffect(() => {
    setPreview(null);
    setReplace(false);
  }, [folder, versionId]);

  const versionOptions = useMemo(
    () => [
      { value: 0, label: t("Unknown"), hint: t("Do not match a server version") },
      ...versions.map((version) => ({
        value: version.id,
        label: version.version ? `v${version.version}` : version.fileName,
      })),
    ],
    [versions],
  );
  // The stored label is the server version without the display "v" prefix.
  const versionLabel = versions.find((version) => version.id === versionId)?.version ?? null;

  const browse = async () => {
    try {
      const picked = await native.pickFolder(t("Choose the existing game folder"));
      if (picked) setFolder(picked);
    } catch (cause) {
      push({ title: t("Could not open the folder picker"), description: errorMessage(cause), tone: "danger" });
    }
  };

  const check = async () => {
    if (!folder.trim()) return;
    setBusy("check");
    setError(null);
    try {
      const next = await native.adoptInstallation({
        gameId,
        folder: folder.trim(),
        versionId,
        version: versionLabel,
        executable: executable.trim() || null,
        title,
      });
      setPreview(next);
    } catch (cause) {
      const message = errorMessage(cause);
      setError(message);
      push({ title: t("Could not inspect the folder"), description: message, tone: "danger" });
    } finally {
      setBusy(null);
    }
  };

  const needsReplace = Boolean(preview?.existing && preview.existing.source !== "adopted");
  const canAdopt = Boolean(preview) && (!needsReplace || replace);

  const adopt = async () => {
    if (!preview || !canAdopt) return;
    setBusy("adopt");
    setError(null);
    try {
      const record = await native.confirmAdoptInstallation({
        gameId,
        folder: preview.folder,
        versionId: preview.versionId,
        version: preview.version,
        executable: executable.trim() || preview.executable,
        replace: needsReplace ? replace : false,
        title,
      });
      await queryClient.invalidateQueries({ queryKey: ["installed"] });
      await queryClient.invalidateQueries({ queryKey: ["recent-games"] });
      await queryClient.invalidateQueries({ queryKey: ["game", gameId] });
      await queryClient.invalidateQueries({ queryKey: ["games"] });
      push({
        title: t("{title} is linked", { title }),
        description: t("Ludex now starts {title} from {folder}. Nothing was copied or moved.", {
          title,
          folder: record.installPath,
        }),
        tone: "success",
      });
      onAdopted?.();
      onOpenChange(false);
    } catch (cause) {
      const message = errorMessage(cause);
      setError(message);
      push({ title: t("Could not link the folder"), description: message, tone: "danger" });
    } finally {
      setBusy(null);
    }
  };

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title={t("Link an existing installation")}
      description={t("Point Ludex at a game folder that is already on this machine, for example a GameVault download. Nothing is downloaded, copied or moved.")}
      footer={
        <>
          <Button variant="ghost" onClick={() => onOpenChange(false)} disabled={busy !== null}>
            {t("Cancel")}
          </Button>
          {preview ? (
            <Button variant="primary" loading={busy === "adopt"} disabled={!canAdopt} onClick={() => void adopt()}>
              {t("Link folder")}
            </Button>
          ) : (
            <Button variant="primary" loading={busy === "check"} disabled={!folder.trim()} onClick={() => void check()}>
              {t("Check folder")}
            </Button>
          )}
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <Field
          label={t("Game folder")}
          htmlFor="adopt-folder"
          hint={t("An absolute path to the folder that contains the game files.")}
        >
          <div className="flex items-stretch gap-2">
            <Input
              id="adopt-folder"
              value={folder}
              onChange={(event) => setFolder(event.target.value)}
              placeholder={
                typeof navigator !== "undefined" && /win/i.test(navigator.userAgent)
                  ? "D:\\GameVault\\Hades"
                  : "/mnt/games/GameVault/Hades"
              }
              spellCheck={false}
              disabled={busy !== null}
              className="h-10 font-mono text-[11.5px]"
            />
            <Button
              variant="default"
              size="sm"
              className="h-10 shrink-0"
              icon={<FolderOpen className="size-3.5" />}
              onClick={() => void browse()}
              disabled={busy !== null || !isNative()}
            >
              {t("Browse")}
            </Button>
          </div>
        </Field>

        <Field label={t("Matched version")} htmlFor="adopt-version" hint={t("Choose a server version for updates and metadata, or leave it unknown. Ludex never guesses a version from the title.")}>
          <div className="w-[240px]">
            <Select
              id="adopt-version"
              label={t("Matched version")}
              value={versionId}
              onChange={setVersionId}
              options={versionOptions}
              disabled={busy !== null}
            />
          </div>
        </Field>

        <Field
          label={t("Executable")}
          htmlFor="adopt-executable"
          hint={t("Leave empty to use the detected executable. You can also choose one of the files found, or type a path inside the folder.")}
        >
          <Input
            id="adopt-executable"
            value={executable}
            onChange={(event) => setExecutable(event.target.value)}
            placeholder={preview?.executable ?? t("Detected automatically")}
            spellCheck={false}
            disabled={busy !== null}
            className="h-10 font-mono text-[11.5px]"
          />
        </Field>

        {preview && preview.executableCandidates.length > 0 && (
          <Field
            label={t("Executables found")}
            htmlFor="adopt-executable-candidates"
            hint={t("Only the selected one is launched; nothing else is changed.")}
          >
            <Select
              id="adopt-executable-candidates"
              label={t("Executables found")}
              value={executable || preview.executable || ""}
              onChange={setExecutable}
              options={preview.executableCandidates.map((path) => ({
                value: path,
                label: relativeToFolder(path, preview.folder),
              }))}
              disabled={busy !== null}
            />
          </Field>
        )}

        {preview && (
          <div className="flex flex-col gap-3 border border-line bg-surface/50 p-3">
            <div className="flex items-center justify-between gap-3">
              <span className="flex items-center gap-2 text-[12.5px] text-ink">
                <Link2 className="size-3.5 text-ink-3" aria-hidden="true" />
                {t("Folder checked")}
              </span>
              <Badge tone={preview.executable ? "ok" : "signal"}>
                {preview.executable ? t("Executable found") : t("No executable")}
              </Badge>
            </div>
            <dl className="flex flex-col gap-1.5 text-[12px]">
              <div className="flex items-baseline justify-between gap-4">
                <dt className="shrink-0 text-ink-3">{t("Folder")}</dt>
                <dd className="truncate font-mono text-[11px] text-ink-2" title={preview.folder}>
                  {preview.folder}
                </dd>
              </div>
              <div className="flex items-baseline justify-between gap-4">
                <dt className="shrink-0 text-ink-3">{t("Executable")}</dt>
                <dd className="truncate font-mono text-[11px] text-ink-2" title={preview.executable ?? ""}>
                  {preview.executable ?? t("None found")}
                </dd>
              </div>
              <div className="flex items-baseline justify-between gap-4">
                <dt className="shrink-0 text-ink-3">{t("Size on disk")}</dt>
                <dd className="num flex items-center gap-2 text-[12px] text-ink-2">
                  <HardDrive className="size-3.5 text-ink-3" aria-hidden="true" />
                  {formatBytes(preview.sizeBytes)}
                  <span className="text-ink-3">{tn(preview.fileCount, "1 file", "{count} files")}</span>
                </dd>
              </div>
            </dl>

            {preview.warnings.map((warning) => (
              <p key={warning} className="text-[11.5px] leading-relaxed text-signal-text">
                {warning}
              </p>
            ))}

            {needsReplace && preview.existing && (
              <div className="flex flex-col gap-2 border-l-2 border-danger pl-3">
                <p className="text-[12px] leading-relaxed text-ink-2">
                  {t("“{title}” is already {source}. Linking this folder replaces that Ludex entry. Its files are left alone.", {
                    title: preview.existing.title,
                    source: preview.existing.source === "ludex" ? t("installed through Ludex") : t("imported from {source}", { source: preview.existing.source }),
                  })}
                </p>
                <label className="flex items-center gap-2.5 text-[12px] text-ink-2">
                  <Checkbox
                    checked={replace}
                    onCheckedChange={setReplace}
                    label={t("Replace the existing entry")}
                  />
                  {t("Replace the existing entry")}
                </label>
              </div>
            )}
          </div>
        )}

        {error && (
          <p role="alert" className="border-l-2 border-danger pl-3 text-[12.5px] text-danger">
            {error}
          </p>
        )}

        <p className="text-[11.5px] leading-relaxed text-ink-3">
          {t("Ludex keeps the folder where it is, launches the executable from it and records playtime like any other install. The game page then offers Play, Open folder, Build manifest, Remove from Ludex and Uninstall and delete files.")}
        </p>
      </div>
    </Dialog>
  );
}
