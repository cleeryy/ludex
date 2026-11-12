import { Link, useNavigate } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Download, Loader2 } from "lucide-react";
import { api } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { t } from "@/lib/i18n";
import { deviceLabel, isRemoteActive, remotePercent, remoteStatusLabel, remoteStatusTone } from "@/lib/mobile";
import { isNative } from "@/lib/native";
import type { RemoteInstall } from "@/lib/types";
import { useAuth } from "@/state/auth";
import { useToast } from "@/state/toasts";
import { Badge, Button } from "@/components/ui/controls";

/**
 * "Install on the desktop" action for the browser/mobile client (#167).
 *
 * The phone cannot install a game, so it queues a request for a desktop signed
 * into the same account. The button reflects the request the account already
 * has for this game: queue, follow progress, or "ready".
 */
export function RemoteInstallAction({
  gameId,
  versionId,
  title,
}: {
  gameId: number;
  versionId?: number | null;
  title: string;
}) {
  const { user } = useAuth();
  const { push } = useToast();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const enabled = !isNative() && Boolean(user);

  const { data } = useQuery({
    queryKey: ["remote-installs"],
    queryFn: () => api.listRemoteInstalls(),
    enabled,
    staleTime: 15_000,
  });

  const install: RemoteInstall | undefined = data?.find((row) => row.gameId === gameId);

  const queue = useMutation({
    mutationFn: () =>
      api.createRemoteInstall({
        gameId,
        versionId: versionId ?? null,
        device: typeof navigator === "undefined" ? null : deviceLabel(navigator.userAgent),
      }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["remote-installs"] });
      push({
        title: t("Install queued"),
        description: t("{title} will install on the desktop when the app opens.", { title }),
        tone: "success",
      });
      navigate("/remote");
    },
    onError: (cause) =>
      push({ title: t("Could not queue the install"), description: errorMessage(cause), tone: "danger" }),
  });

  if (!enabled) return null;

  if (!install) {
    return (
      <Button
        variant="default"
        size="lg"
        icon={<Download className="size-4" />}
        loading={queue.isPending}
        onClick={() => queue.mutate()}
      >
        {t("Install on the desktop")}
      </Button>
    );
  }

  const percent = remotePercent(install);

  return (
    <div className="flex flex-wrap items-center gap-2">
      <Link to="/remote" className="inline-flex items-center gap-2">
        <Badge tone={remoteStatusTone(install.status)}>{remoteStatusLabel(install.status)}</Badge>
        {percent !== null && (
          <span className="num text-[12px] text-ink-3">
            {isRemoteActive(install.status) && (
              <Loader2 className="me-1 inline size-3 animate-spin text-signal" aria-hidden="true" />
            )}
            {percent}%
          </span>
        )}
        <span className="text-[12.5px] text-ink-2 underline decoration-line-2 underline-offset-2">
          {isRemoteActive(install.status) ? t("Follow progress") : t("Open the queue")}
        </span>
      </Link>
    </div>
  );
}
