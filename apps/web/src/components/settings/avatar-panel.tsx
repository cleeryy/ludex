import { useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { ImagePlus, Upload } from "lucide-react";
import { useAuth } from "@/state/auth";
import { useToast } from "@/state/toasts";
import { errorMessage } from "@/lib/http";
import { t } from "@/lib/i18n";
import {
  AVATAR_ACCEPT,
  AVATAR_MAX_BYTES,
  avatarFileProblem,
  deleteAvatar,
  uploadAvatar,
} from "@/lib/avatar";
import { UserAvatar } from "@/components/community/user-avatar";
import { SettingsGroup } from "@/components/settings/settings-section";
import { Button } from "@/components/ui/controls";

/**
 * Account avatar block (#183): current preview, file picker, drag/drop,
 * save and remove. The server sniffs the real format; this only keeps obvious
 * mistakes from making a round-trip.
 */
export function AvatarPanel({ displayName }: { displayName: string }) {
  const { user, refreshUser } = useAuth();
  const { push } = useToast();
  const queryClient = useQueryClient();
  const input = useRef<HTMLInputElement>(null);
  const previewUrl = useRef<string | null>(null);
  const [file, setFile] = useState<File | null>(null);
  const [preview, setPreview] = useState<string | null>(null);
  const [problem, setProblem] = useState<"type" | "size" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<"save" | "remove" | null>(null);
  const [dragging, setDragging] = useState(false);

  useEffect(
    () => () => {
      if (previewUrl.current) URL.revokeObjectURL(previewUrl.current);
      previewUrl.current = null;
    },
    [],
  );

  const choose = (next: File | undefined) => {
    if (!next) return;
    setError(null);
    const issue = avatarFileProblem(next);
    if (issue) {
      if (previewUrl.current) URL.revokeObjectURL(previewUrl.current);
      previewUrl.current = null;
      setFile(null);
      setPreview(null);
      setProblem(issue);
      if (input.current) input.current.value = "";
      return;
    }
    setProblem(null);
    if (previewUrl.current) URL.revokeObjectURL(previewUrl.current);
    previewUrl.current = URL.createObjectURL(next);
    setFile(next);
    setPreview(previewUrl.current);
  };

  const clearChoice = () => {
    if (previewUrl.current) URL.revokeObjectURL(previewUrl.current);
    previewUrl.current = null;
    setFile(null);
    setPreview(null);
    setProblem(null);
    setError(null);
    if (input.current) input.current.value = "";
  };

  const invalidate = async () => {
    await refreshUser();
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: ["my-profile"] }),
      queryClient.invalidateQueries({ queryKey: ["community-users"] }),
      queryClient.invalidateQueries({ queryKey: ["community-feed"] }),
      queryClient.invalidateQueries({ queryKey: ["community-user", user?.id] }),
    ]);
  };

  const save = async () => {
    if (!file) return;
    setBusy("save");
    setError(null);
    try {
      await uploadAvatar(file);
      await invalidate();
      clearChoice();
      push({ title: t("Avatar updated"), tone: "success" });
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(null);
    }
  };

  const remove = async () => {
    setBusy("remove");
    setError(null);
    try {
      await deleteAvatar();
      await invalidate();
      clearChoice();
      push({ title: t("Avatar removed"), tone: "success" });
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(null);
    }
  };

  const name = displayName || user?.username || "";

  return (
    <SettingsGroup
      title={t("Avatar")}
      description={t("Your logo appears in the sidebar, on your public profile and next to your activity.")}
    >
      <div className="flex flex-wrap items-start gap-5">
        <div className="flex flex-col items-center gap-2">
          <UserAvatar name={name} src={preview ?? user?.avatarUrl} size={72} />
          <span className="font-mono text-[10px] text-ink-3">
            {file ? t("New avatar") : t("Current avatar")}
          </span>
        </div>

        <div className="flex min-w-0 flex-1 flex-col gap-3">
          <div
            onDragOver={(event) => {
              event.preventDefault();
              setDragging(true);
            }}
            onDragLeave={() => setDragging(false)}
            onDrop={(event) => {
              event.preventDefault();
              setDragging(false);
              choose(event.dataTransfer.files[0]);
            }}
            className={`flex flex-wrap items-center gap-3 rounded-[2px] border border-dashed px-3 py-3 transition-colors ${
              dragging ? "border-signal bg-surface" : "border-line bg-surface/40"
            }`}
          >
            <ImagePlus className="size-4 shrink-0 text-ink-3" aria-hidden="true" />
            <p className="min-w-0 flex-1 text-[12px] leading-relaxed text-ink-3">
              {t("Drop an image here, or choose a file. PNG, JPEG or WebP, up to 2 MiB.")}
            </p>
            <input
              ref={input}
              type="file"
              accept={AVATAR_ACCEPT}
              className="sr-only"
              aria-label={t("Choose image")}
              onChange={(event) => choose(event.target.files?.[0])}
            />
            <Button variant="ghost" size="sm" icon={<Upload className="size-3.5" />} onClick={() => input.current?.click()}>
              {t("Choose image")}
            </Button>
          </div>

          {problem && (
            <p role="alert" className="border-l-2 border-danger pl-3 text-[12.5px] text-danger">
              {problem === "type" ? t("Use a PNG, JPEG or WebP image.") : t("The image must be at most 2 MiB.")}
            </p>
          )}
          {error && (
            <p role="alert" className="border-l-2 border-danger pl-3 text-[12.5px] text-danger">
              {error}
            </p>
          )}

          <p className="text-[11.5px] leading-relaxed text-ink-3">
            {t("Uploaded images are served from a public media URL: anyone with the link can view them, even when your profile is private.")}
          </p>

          <div className="flex flex-wrap items-center gap-2">
            {file && (
              <>
                <Button variant="default" loading={busy === "save"} onClick={() => void save()}>
                  {t("Save avatar")}
                </Button>
                <Button variant="ghost" disabled={busy !== null} onClick={clearChoice}>
                  {t("Cancel")}
                </Button>
              </>
            )}
            {!file && user?.avatarUrl && (
              <Button variant="ghost" loading={busy === "remove"} onClick={() => void remove()}>
                {t("Remove avatar")}
              </Button>
            )}
            <span className="font-mono text-[10px] text-ink-3">
              {AVATAR_MAX_BYTES / (1024 * 1024)} MiB
            </span>
          </div>
        </div>
      </div>
    </SettingsGroup>
  );
}
