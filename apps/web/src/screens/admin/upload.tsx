import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { CheckCircle2, FileArchive, UploadCloud, X } from "lucide-react";
import { api } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { formatBytes, formatPercent } from "@/lib/format";
import type { CompleteUploadResponse, UploadSession } from "@/lib/types";
import { useToast } from "@/state/toasts";
import { ErrorState, PageHeader, SectionHeader } from "@/components/page";
import { Badge, Button, Progress } from "@/components/ui/controls";
import { t } from "@/lib/i18n";

const STORAGE_KEY = "ludex.upload";

type StoredUpload = { id: string; fileName: string; size: number };

type Phase = "idle" | "uploading" | "verifying" | "complete" | "error";

function readStored(): StoredUpload | null {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as StoredUpload;
    return parsed?.id ? parsed : null;
  } catch {
    return null;
  }
}

function store(value: StoredUpload | null) {
  if (value) localStorage.setItem(STORAGE_KEY, JSON.stringify(value));
  else localStorage.removeItem(STORAGE_KEY);
}

export function UploadScreen() {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const inputRef = useRef<HTMLInputElement>(null);
  const cancelRef = useRef(false);

  const [file, setFile] = useState<File | null>(null);
  const [session, setSession] = useState<UploadSession | null>(null);
  const [phase, setPhase] = useState<Phase>("idle");
  const [error, setError] = useState<string | null>(null);
  const [uploaded, setUploaded] = useState(0);
  const [restoring, setRestoring] = useState(true);
  const [result, setResult] = useState<CompleteUploadResponse | null>(null);
  const [dragging, setDragging] = useState(false);

  // Resume after a reload: the server is the source of truth for the stored id.
  useEffect(() => {
    const stored = readStored();
    if (!stored) {
      setRestoring(false);
      return;
    }
    let alive = true;
    api
      .getUpload(stored.id)
      .then((upload) => {
        if (!alive) return;
        if (upload.status === "open") {
          setSession(upload);
          setUploaded(upload.receivedBytes);
        } else {
          store(null);
        }
      })
      .catch(() => {
        if (alive) store(null);
      })
      .finally(() => {
        if (alive) setRestoring(false);
      });
    return () => {
      alive = false;
    };
  }, []);

  const choose = (next: File | null) => {
    if (!next) return;
    const name = next.name.toLowerCase();
    if (!/\.(zip|rar|7z)$/.test(name)) {
      setError(t("Choose a .zip, .rar or .7z archive."));
      return;
    }
    if (session && (next.size !== session.size || next.name !== session.fileName)) {
      setError(t("This is not the file that started the upload. Pick the same archive to resume."));
      return;
    }
    setError(null);
    setFile(next);
    if (!session) {
      setPhase("idle");
      setUploaded(0);
    }
  };

  const start = async () => {
    if (!file) return;
    cancelRef.current = false;
    setError(null);
    setResult(null);
    setPhase("uploading");
    try {
      let active = session;
      if (!active) {
        active = await api.initUpload({ fileName: file.name, size: file.size });
        store({ id: active.id, fileName: active.fileName, size: active.size });
        setSession(active);
        setUploaded(active.receivedBytes);
      }
      const received = new Set(active.receivedParts);
      for (let part = 0; part < active.totalParts; part += 1) {
        if (cancelRef.current) throw new Error("cancelled");
        if (received.has(part)) continue;
        const start = part * active.chunkSize;
        const end = Math.min((part + 1) * active.chunkSize, active.size);
        const chunk = file.slice(start, end);
        await api.uploadPart(active.id, part, chunk, (loaded) => {
          setUploaded(start + loaded);
        });
        setUploaded(end);
      }
      setPhase("verifying");
      const completed = await api.completeUpload(active.id);
      store(null);
      setSession(null);
      setFile(null);
      setResult(completed);
      setPhase("complete");
      void queryClient.invalidateQueries({ queryKey: ["games"] });
      void queryClient.invalidateQueries({ queryKey: ["stats"] });
      void queryClient.invalidateQueries({ queryKey: ["scan"] });
      push({ title: t("Upload complete"), description: t("{file} is now in the library.", { file: completed.fileName }), tone: "success" });
    } catch (cause) {
      if (cause instanceof Error && cause.message === "cancelled") {
        setPhase("idle");
        return;
      }
      setPhase("error");
      setError(errorMessage(cause));
    }
  };

  const cancel = async () => {
    cancelRef.current = true;
    if (session) {
      try {
        await api.cancelUpload(session.id);
      } catch {
        // The upload may already be gone; clearing local state is enough.
      }
    }
    store(null);
    setSession(null);
    setFile(null);
    setUploaded(0);
    setPhase("idle");
    setError(null);
  };

  const total = session?.size ?? file?.size ?? 0;
  const percent = total > 0 ? Math.min(100, (uploaded / total) * 100) : 0;
  const busy = phase === "uploading" || phase === "verifying";

  return (
    <div className="mx-auto w-full max-w-[880px] px-4 pb-16 sm:px-6">
      <PageHeader
        title={t("Upload a game")}
        description={t("Send an archive straight into the library. Chunks are resumable: if the connection drops or the tab reloads, pick the same file again to continue.")}
      />

      <section className="mt-8">
        <SectionHeader title={t("Archive")} description={t("ZIP, RAR or 7z")} />
        {restoring ? (
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
                  <Button
                    variant="ghost"
                    onClick={() => {
                      setResult(null);
                      setPhase("idle");
                    }}
                  >
                    {t("Upload another")}
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
              onClick={() => !busy && inputRef.current?.click()}
              onKeyDown={(event) => {
                if ((event.key === "Enter" || event.key === " ") && !busy) inputRef.current?.click();
              }}
              onDragOver={(event) => {
                event.preventDefault();
                if (!busy) setDragging(true);
              }}
              onDragLeave={() => setDragging(false)}
              onDrop={(event) => {
                event.preventDefault();
                setDragging(false);
                if (!busy) choose(event.dataTransfer.files[0] ?? null);
              }}
              className="flex cursor-pointer flex-col items-center justify-center gap-2 border border-dashed border-line-2 bg-surface px-6 py-12 text-center transition-colors hover:border-line-3"
              data-dragging={dragging}
            >
              <UploadCloud className="size-6 text-ink-3" aria-hidden="true" />
              <p className="text-[14px] text-ink">
                {session ? `Resuming ${session.fileName}` : t("Drop an archive here, or click to browse")}
              </p>
              <p className="text-[12.5px] text-ink-3">
                {session
                  ? t("Select the same file again to continue where the last attempt stopped.")
                  : t("The server verifies the size and sha256 (when provided) before indexing.")}
              </p>
              <input
                ref={inputRef}
                type="file"
                accept=".zip,.rar,.7z"
                className="hidden"
                onChange={(event) => choose(event.target.files?.[0] ?? null)}
              />
            </div>

            {file && (
              <div className="mt-4 flex flex-wrap items-center gap-3 border border-line bg-surface px-4 py-3">
                <FileArchive className="size-4 text-ink-3" aria-hidden="true" />
                <div className="min-w-0 flex-1">
                  <p className="truncate text-[13px] text-ink">{file.name}</p>
                  <p className="num text-[11.5px] text-ink-3">
                    {formatBytes(file.size)}
                    {session && ` · ${session.receivedParts.length}/${session.totalParts} chunks stored`}
                  </p>
                </div>
                <Badge tone={busy ? "signal" : "outline"}>
                  {phase === "uploading" ? t("Uploading") : phase === "verifying" ? t("Verifying") : session ? t("Resumable") : t("Ready")}
                </Badge>
              </div>
            )}

            {(busy || (session && !file)) && (
              <div className="mt-4">
                <Progress value={percent} label={phase === "verifying" ? t("Verifying and indexing…") : t("Uploading")} />
                <p className="num mt-1 text-[11.5px] text-ink-3">
                  {formatBytes(uploaded)} / {formatBytes(total)} · {formatPercent(percent, 1)}
                </p>
              </div>
            )}

            {error && (
              <div className="mt-4">
                <ErrorState title={t("Upload failed")} message={error} />
              </div>
            )}

            <div className="mt-5 flex flex-wrap gap-2">
              <Button
                variant="primary"
                icon={<UploadCloud className="size-4" />}
                disabled={!file || busy}
                onClick={() => void start()}
              >
                {session ? t("Resume upload") : t("Start upload")}
              </Button>
              {(session || file) && (
                <Button variant="ghost" icon={<X className="size-4" />} disabled={phase === "verifying"} onClick={() => void cancel()}>
                  {t("Cancel")}
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
