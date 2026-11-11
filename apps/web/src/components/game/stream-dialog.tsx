import { useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { AlertTriangle, Cast, Settings2, Wifi } from "lucide-react";
import { errorMessage } from "@/lib/http";
import { isNative, native } from "@/lib/native";
import { useToast } from "@/state/toasts";
import { cn } from "@/lib/cn";
import { Badge, Button, Skeleton } from "@/components/ui/controls";
import { Dialog } from "@/components/ui/dialog";
import { t } from "@/lib/i18n";

/**
 * Stream a published Ludex game through the local Moonlight client (#98).
 *
 * Sunshine hosts the game; Moonlight plays it. Ludex only resolves the app name
 * it published and starts `moonlight stream <host> <app>`.
 */
export function StreamDialog({
  gameId,
  title,
  open,
  onOpenChange,
}: {
  gameId: number;
  title: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const navigate = useNavigate();
  const { push } = useToast();
  const [selected, setSelected] = useState<string | null>(null);
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const target = useQuery({
    queryKey: ["stream-target", gameId],
    queryFn: () => native.streamTarget(gameId),
    enabled: open && isNative(),
    retry: false,
    staleTime: 10_000,
  });

  useEffect(() => {
    if (!open) {
      setSelected(null);
      setError(null);
      setStarting(false);
    }
  }, [open]);

  useEffect(() => {
    const hosts = target.data?.hosts ?? [];
    if (selected && hosts.some((host) => host.address === selected)) return;
    const first = hosts.find((host) => host.paired) ?? hosts[0];
    setSelected(first?.address ?? null);
  }, [target.data, selected]);

  const selectedHost = target.data?.hosts.find((host) => host.address === selected) ?? null;

  const start = async () => {
    if (!selected) return;
    setStarting(true);
    setError(null);
    try {
      const launch = await native.streamGame(gameId, selected);
      push({ title: t("Moonlight is starting"), description: launch.message, tone: "success" });
      onOpenChange(false);
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setStarting(false);
    }
  };

  const openSettings = () => {
    onOpenChange(false);
    navigate("/settings");
  };

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title={t("Stream {title}", { title })}
      description={t("Moonlight opens the stream from the Sunshine host that published this game.")}
      footer={
        <div className="flex flex-wrap justify-end gap-2">
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            {t("Cancel")}
          </Button>
          {target.data?.published && selectedHost?.paired && (
            <Button variant="primary" loading={starting} icon={<Cast className="size-4" />} onClick={() => void start()}>
              {t("Start stream")}
            </Button>
          )}
        </div>
      }
    >
      {target.isPending && (
        <div className="flex flex-col gap-2">
          <Skeleton className="h-5 w-2/3" />
          <Skeleton className="h-12 w-full" />
        </div>
      )}

      {target.isError && (
        <p role="alert" className="border-l-2 border-danger pl-3 text-[12.5px] text-danger">
          {errorMessage(target.error)}
        </p>
      )}

      {target.data && !target.data.published && (
        <div className="flex flex-col gap-3">
          <p className="flex items-start gap-2 border-l-2 border-warn/60 pl-3 text-[12.5px] leading-relaxed text-ink-2">
            <AlertTriangle className="mt-0.5 size-3.5 shrink-0 text-warn" aria-hidden="true" />
            <span>{target.data.message}</span>
          </p>
          <Button variant="default" size="sm" icon={<Settings2 className="size-3.5" />} onClick={openSettings}>
            {t("Open streaming settings")}
          </Button>
        </div>
      )}

      {target.data?.published && (
        <div className="flex flex-col gap-4">
          <dl className="border-t border-line">
            <div className="flex items-baseline justify-between gap-4 border-b border-line py-2">
              <dt className="text-[12px] text-ink-3">{t("Sunshine app")}</dt>
              <dd className="truncate text-[12.5px] text-ink">{target.data.appName}</dd>
            </div>
            <div className="flex items-baseline justify-between gap-4 border-b border-line py-2">
              <dt className="text-[12px] text-ink-3">{t("Moonlight")}</dt>
              <dd className={cn("truncate font-mono text-[10.5px]", target.data.moonlight.detected ? "text-ink-2" : "text-warn")}>
                {target.data.moonlight.binary ?? t("not found")}
              </dd>
            </div>
          </dl>

          {!target.data.moonlight.detected && (
            <p className="border-l-2 border-danger pl-3 text-[12px] leading-relaxed text-ink-2">
              {target.data.moonlight.message}
            </p>
          )}

          {target.data.moonlight.detected && target.data.hosts.length === 0 && (
            <p className="border-l-2 border-warn/60 pl-3 text-[12px] leading-relaxed text-ink-2">
              {target.data.moonlight.message}
            </p>
          )}

          {target.data.hosts.length > 0 && (
            <div>
              <p className="mb-2 text-[12px] text-ink-3">{t("Choose a host")}</p>
              <div className="flex flex-col gap-1.5">
                {target.data.hosts.map((host) => (
                  <button
                    key={`${host.name}-${host.address}`}
                    type="button"
                    aria-pressed={selected === host.address}
                    disabled={!host.paired}
                    onClick={() => setSelected(host.address)}
                    className={cn(
                      "flex flex-wrap items-center gap-x-3 gap-y-1 border px-3 py-2 text-left transition-colors",
                      selected === host.address ? "border-signal bg-surface-2" : "border-line hover:border-line-2",
                      !host.paired && "opacity-50",
                    )}
                  >
                    <Wifi className="size-3.5 shrink-0 text-ink-3" aria-hidden="true" />
                    <span className="text-[13px] text-ink">{host.name}</span>
                    <span className="font-mono text-[10.5px] text-ink-3">{host.address}</span>
                    <span className="ml-auto">
                      <Badge tone={host.paired ? "ok" : "outline"}>{host.paired ? t("paired") : t("not paired")}</Badge>
                    </span>
                  </button>
                ))}
              </div>
              <p className="mt-2 text-[11px] leading-relaxed text-ink-3">
                {t("Pairing happens once in Moonlight. An unpaired host is listed but cannot stream yet.")}
              </p>
            </div>
          )}

          {error && (
            <p role="alert" className="border-l-2 border-danger pl-3 text-[12.5px] text-danger">
              {error}
            </p>
          )}
        </div>
      )}
    </Dialog>
  );
}
