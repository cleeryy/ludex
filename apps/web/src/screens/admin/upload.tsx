import { useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { CheckCircle2, FileArchive, FilePlus2, UploadCloud, X } from "lucide-react";
import { formatBytes, formatPercent } from "@/lib/format";
import { useUpload } from "@/state/upload";
import { ErrorState, PageHeader, SectionHeader } from "@/components/page";
import { Badge, Button, Progress } from "@/components/ui/controls";
import { t } from "@/lib/i18n";

/**
 * View/controller of the app-wide upload (#185). The chunk loop and the live
 * state live in `UploadProvider`; this screen only renders them, so leaving the
 * route never interrupts the transfer and coming back shows it unchanged.
 */
export function UploadScreen() {
  const navigate = useNavigate();
  const inputRef = useRef<HTMLInputElement>(null);
  const [pickError, setPickError] = useState<string | null>(null);
  const [dragging, setDragging] = useState(false);
  /** Archive chosen while a transfer runs: the two-button choice below acts on it. */
  const [blocked, setBlocked] = useState<File | null>(null);
  const upload = useUpload();
  const {
    ready,
    phase,
    file,
    session,
    uploaded,
    total,
    percent,
    error,
    result,
    awaitingFile,
    active,
    selectFile,
    start,
    resume,
    cancel,
    dismissResult,
  } = upload;

  const openPicker = () => inputRef.current?.click();

  const choose = async (next: File | null) => {
    if (!next) return;
    // A restored session has no File: picking the same archive continues from
    // the parts the server already holds, without a second click.
    const outcome = awaitingFile ? await resume(next) : selectFile(next);
    if (outcome === "invalid-type") {
      setPickError(t("Choose a .zip, .rar or .7z archive."));
    } else if (outcome === "mismatch") {
      setPickError(t("This is not the file that started the upload. Pick the same archive to resume."));
    } else if (outcome === "busy") {
      // A transfer is already running: offer cancel/finish instead of a dead end.
      setBlocked(next);
    } else {
      setPickError(null);
      setBlocked(null);
    }
    // Re-picking the same archive must fire `change` even if the value is equal.
    if (inputRef.current) inputRef.current.value = "";
  };

  /** Second-start choice: drop the running transfer, then send this archive. */
  const replace = async () => {
    const next = blocked;
    if (!next) return;
    setBlocked(null);
    setPickError(null);
    await cancel();
    if (selectFile(next) === "selected") await start();
  };

  const begin = async () => {
    const outcome = await start();
    if (outcome === "busy") {
      setPickError(t("An upload is already running."));
    } else {
      setPickError(null);
    }
  };

  const stop = async () => {
    setPickError(null);
    setBlocked(null);
    await cancel();
  };

  const reset = () => {
    setPickError(null);
    setBlocked(null);
    dismissResult();
  };

  return (
    <div className="mx-auto w-full max-w-[880px] px-4 pb-16 sm:px-6">
      <PageHeader
        title={t("Upload a game")}
        description={t("Send an archive straight into the library. Chunks are resumable: if the connection drops or the tab reloads, pick the same file again to continue.")}
      />

      <section className="mt-8">
        <SectionHeader title={t("Archive")} description={t("ZIP, RAR or 7z")} />
        <input
          ref={inputRef}
          type="file"
          accept=".zip,.rar,.7z"
          className="hidden"
          onChange={(event) => void choose(event.target.files?.[0] ?? null)}
        />
        {!ready ? (
          <p className="text-[13px] text-ink-3">{t("Checking for unfinished uploads…")}</p>
        ) : result ? (
          <div className="border border-line bg-surface p-6">
            <div className="flex items-start gap-3">
              <CheckCircle2 className="mt-0.5 size-5 text-ok" aria-hidden="true" />
              <div className="min-w-0">
                <h3 className="text-[15px] font-medium text-ink">{t("Upload complete")}</h3>
                <p className="mt-1 text-[13px] text-ink-2">
                  {t("{file} · {size} · indexed as game #{id}", { file: result.fileName, size: formatBytes(result.size), id: result.gameId })}
                </p>
                <div className="mt-4 flex flex-wrap gap-2">
                  <Button variant="primary" onClick={() => navigate(`/game/${result.gameId}`)}>
                    {t("Open game")}
                  </Button>
                  <Button variant="ghost" onClick={reset}>
                    {t("Upload another")}
                  </Button>
                </div>
              </div>
            </div>
          </div>
        ) : awaitingFile && session ? (
          <div className="border border-line bg-surface p-6">
            <div className="flex items-start gap-3">
              <FileArchive className="mt-0.5 size-5 text-ink-3" aria-hidden="true" />
              <div className="min-w-0">
                <h3 className="text-[15px] font-medium text-ink">{t("Upload did not finish")}</h3>
                <p className="mt-1 truncate text-[13px] text-ink-2">{session.fileName}</p>
                <p className="num mt-1 text-[11.5px] text-ink-3">
                  {t("The server still has {received} of {size}. Pick the same archive to continue.", {
                    received: formatBytes(session.receivedBytes),
                    size: formatBytes(session.size),
                  })}
                </p>
                <div className="mt-4 flex flex-wrap gap-2">
                  <Button variant="primary" icon={<UploadCloud className="size-4" />} onClick={openPicker}>
                    {t("Resume upload")}
                  </Button>
                  <Button variant="ghost" icon={<X className="size-4" />} onClick={() => void stop()}>
                    {t("Cancel")}
                  </Button>
                </div>
              </div>
            </div>
          </div>
        ) : (
          <>
            <div
              role="button"
              tabIndex={0}
              aria-disabled={active}
              onClick={() => !active && openPicker()}
              onKeyDown={(event) => {
                if ((event.key === "Enter" || event.key === " ") && !active) openPicker();
              }}
              onDragOver={(event) => {
                event.preventDefault();
                if (!active) setDragging(true);
              }}
              onDragLeave={() => setDragging(false)}
              onDrop={(event) => {
                event.preventDefault();
                setDragging(false);
                if (!active) void choose(event.dataTransfer.files[0] ?? null);
              }}
              className="flex cursor-pointer flex-col items-center justify-center gap-2 border border-dashed border-line-2 bg-surface px-6 py-12 text-center transition-colors hover:border-line-3 aria-disabled:cursor-default aria-disabled:opacity-60"
              data-dragging={dragging}
            >
              <UploadCloud className="size-6 text-ink-3" aria-hidden="true" />
              <p className="text-[14px] text-ink">
                {active ? t("Uploading") : session ? t("Resume upload") : t("Drop an archive here, or click to browse")}
              </p>
              <p className="text-[12.5px] text-ink-3">
                {active
                  ? t("You can leave this page; the upload keeps running.")
                  : session
                    ? t("Select the same file again to continue where the last attempt stopped.")
                    : t("The server verifies the size and sha256 (when provided) before indexing.")}
              </p>
            </div>

            {file && (
              <div className="mt-4 flex flex-wrap items-center gap-3 border border-line bg-surface px-4 py-3">
                <FileArchive className="size-4 text-ink-3" aria-hidden="true" />
                <div className="min-w-0 flex-1">
                  <p className="truncate text-[13px] text-ink">{file.name}</p>
                  <p className="num text-[11.5px] text-ink-3">
                    {formatBytes(file.size)}
                    {session &&
                      ` · ${t("{stored} of {total} chunks stored", {
                        stored: session.receivedParts.length,
                        total: session.totalParts,
                      })}`}
                  </p>
                </div>
                <Badge tone={active ? "signal" : "outline"}>
                  {phase === "uploading"
                    ? t("Uploading")
                    : phase === "verifying"
                      ? t("Verifying")
                      : session
                        ? t("Resumable")
                        : phase === "error"
                          ? t("Upload failed")
                          : t("Ready")}
                </Badge>
              </div>
            )}

            {(active || (phase === "error" && session)) && (
              <div className="mt-4">
                <Progress
                  value={percent}
                  label={phase === "verifying" ? t("Verifying and indexing…") : t("Uploading")}
                />
                <p className="num mt-1 text-[11.5px] text-ink-3">
                  {formatBytes(uploaded)} / {formatBytes(total)} · {formatPercent(percent, 1)}
                </p>
              </div>
            )}

            {(pickError || error) && (
              <div className="mt-4">
                <ErrorState
                  title={error ? t("Upload failed") : t("Cannot start this upload")}
                  message={pickError ?? error ?? ""}
                />
              </div>
            )}

            {blocked && active && phase !== "verifying" && (
              <div className="mt-4 border border-line-2 bg-surface-2 px-4 py-3">
                <p className="text-[13px] text-ink">{t("An upload is already running.")}</p>
                <p className="mt-1 text-[12.5px] text-ink-2">
                  {t("Cancel it to send {file} instead, or keep the current transfer.", { file: blocked.name })}
                </p>
                <div className="mt-3 flex flex-wrap gap-2">
                  <Button variant="primary" onClick={() => void replace()}>
                    {t("Cancel current and send")}
                  </Button>
                  <Button variant="ghost" onClick={() => setBlocked(null)}>
                    {t("Keep current upload")}
                  </Button>
                </div>
              </div>
            )}

            <div className="mt-5 flex flex-wrap gap-2">
              <Button
                variant="primary"
                icon={<UploadCloud className="size-4" />}
                disabled={!file || active}
                onClick={() => void begin()}
              >
                {session ? t("Resume upload") : t("Start upload")}
              </Button>
              {(session || file) && (
                <Button
                  variant="ghost"
                  icon={<X className="size-4" />}
                  disabled={phase === "verifying"}
                  onClick={() => void stop()}
                >
                  {t("Cancel")}
                </Button>
              )}
              {active && phase !== "verifying" && (
                <Button
                  variant="ghost"
                  icon={<FilePlus2 className="size-4" />}
                  onClick={openPicker}
                >
                  {t("Choose another archive")}
                </Button>
              )}
            </div>
          </>
        )}
      </section>

      <section className="mt-12">
        <SectionHeader title={t("How it works")} description={t("What the server does with the archive")} />
        <ol className="space-y-3 text-[13px] leading-relaxed text-ink-2">
          <li>
            <span className="text-ink">{t("1. Initiate.")}</span>  {t("The server reserves an upload id and a 16 MiB chunk size.")}
          </li>
          <li>
            <span className="text-ink">{t("2. Send chunks.")}</span>  {t("Each chunk is written atomically; sending the same chunk twice is harmless.")}
          </li>
          <li>
            <span className="text-ink">{t("3. Complete.")}</span>  {t("The server assembles the parts, checks the size and optional SHA-256, then indexes the archive as a new game version.")}
          </li>
          <li>
            <span className="text-ink">{t("4. Resume.")}</span>  {t("The upload id is kept in this browser; after a reload, pick the same file and the stored chunks are skipped.")}
          </li>
        </ol>
      </section>
    </div>
  );
}
