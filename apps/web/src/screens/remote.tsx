import { useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  AlertTriangle,
  Download,
  Monitor,
  RefreshCw,
  Smartphone,
  Trash2,
  Wifi,
  WifiOff,
} from "lucide-react";
import { api, USING_MOCKS } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { cn } from "@/lib/cn";
import { formatBytes, formatRelative } from "@/lib/format";
import { t } from "@/lib/i18n";
import {
  deviceLabel,
  isRemoteActive,
  remoteLooksOffline,
  remotePercent,
  remoteStatusLabel,
  remoteStatusTone,
  remoteSummary,
} from "@/lib/mobile";
import type { Device, RemoteInstall } from "@/lib/types";
import { useAuth } from "@/state/auth";
import { useToast } from "@/state/toasts";
import { GameCover } from "@/components/art/cover";
import { EmptyState, PageHeader, SectionHeader } from "@/components/page";
import { Badge, Button, Progress } from "@/components/ui/controls";
import { DevicePicker, deviceSummary, preferredDeviceId, rememberPreferredDevice, useDevices } from "@/components/game/device-picker";

/**
 * Phone-first remote install queue (#167).
 *
 * The phone never installs anything itself: it queues a request and a desktop
 * signed into the same account claims it. This screen is the follow-up view —
 * queued, downloading and ready — plus the honest "PC offline" state instead
 * of an endless spinner.
 */
export function RemoteScreen() {
  const { user } = useAuth();
  const { push } = useToast();
  const queryClient = useQueryClient();

  const { data, isPending, isError, error, refetch, isFetching } = useQuery({
    queryKey: ["remote-installs"],
    queryFn: () => api.listRemoteInstalls(),
    enabled: Boolean(user),
    // Only poll while something is moving: a settled queue is a static list.
    refetchInterval: (query) =>
      query.state.data?.some((install) => isRemoteActive(install.status)) ? 5_000 : false,
  });

  const invalidate = () => queryClient.invalidateQueries({ queryKey: ["remote-installs"] });

  const devices = useDevices(Boolean(user));
  const deviceList = devices.data ?? [];
  const [defaultTarget, setDefaultTarget] = useState<string | null>(() => preferredDeviceId());
  const chooseDefaultTarget = (id: string | null) => {
    setDefaultTarget(id);
    rememberPreferredDevice(id);
  };
  const cancel = useMutation({
    mutationFn: (install: RemoteInstall) => api.cancelRemoteInstall(install.id),
    onSuccess: () => void invalidate(),
    onError: (cause) =>
      push({ title: t("Could not update the request"), description: errorMessage(cause), tone: "danger" }),
  });

  const installed = useMemo(() => data ?? [], [data]);
  const active = installed.filter((install) => isRemoteActive(install.status));
  const history = installed.filter((install) => !isRemoteActive(install.status));
  const summary = remoteSummary(installed);
  const thisDevice = useMemo(
    () => (typeof navigator === "undefined" ? "Web browser" : deviceLabel(navigator.userAgent)),
    [],
  );

  if (!user) {
    return (
      <div className="page page-in">
        <PageHeader title={t("Remote installs")} />
        <div className="mt-8">
          <EmptyState
            title={t("Sign in to queue an install")}
            description={t("Remote installs belong to your account, so the desktop can pick them up.")}
          >
            <Link to="/login">
              <Button variant="primary">{t("Sign in")}</Button>
            </Link>
          </EmptyState>
        </div>
      </div>
    );
  }

  return (
    <div className="page page-in">
      <PageHeader
        title={t("Remote installs")}
        meta={
          <>
            <span className="num">{t("{count} active", { count: summary.active })}</span>
            <span className="num">{t("{count} ready", { count: summary.ready })}</span>
            {summary.failed > 0 && <span className="num text-danger">{t("{count} failed", { count: summary.failed })}</span>}
            <span className="hidden sm:inline">{t("This device: {device}", { device: thisDevice })}</span>
          </>
        }
        description={t("Queue an install here, then open the Ludex desktop app on the same account. The desktop downloads and installs the game; this phone only follows progress.")}
        actions={
          <Button
            variant="ghost"
            size="sm"
            icon={<RefreshCw className={cn("size-3.5", isFetching && "animate-spin")} />}
            onClick={() => void refetch()}
          >
            {t("Refresh")}
          </Button>
        }
      />

      <section className="mt-6 border border-line bg-surface/40 p-4 sm:p-5">
        <div className="flex items-start gap-3">
          <Smartphone className="mt-0.5 size-4 shrink-0 text-signal" aria-hidden="true" />
          <div className="min-w-0 text-[13px] leading-relaxed text-ink-2">
            <p className="font-medium text-ink">{t("The desktop does the install")}</p>
            <p className="mt-1">
              {t("A request waits until the desktop app is open and signed in. Nothing launches or streams from this phone; installs land on the PC.")}
            </p>
          </div>
        </div>
      </section>

      <section className="mt-8" aria-labelledby="remote-devices">
        <SectionHeader
          id="remote-devices"
          title={t("Your PCs")}
          count={deviceList.length}
          description={t("Signed-in Ludex desktops on this account. A PC stays online for 60 seconds after its last heartbeat.")}
          actions={
            <Button
              variant="ghost"
              size="sm"
              icon={<RefreshCw className={cn("size-3.5", devices.isFetching && "animate-spin")} />}
              onClick={() => void devices.refetch()}
            >
              {t("Refresh")}
            </Button>
          }
        />
        {deviceList.length > 0 && (
          <div className="mb-3 flex flex-wrap items-center gap-2">
            <DevicePicker
              devices={deviceList}
              value={deviceList.some((device) => device.id === defaultTarget) ? defaultTarget : null}
              onChange={chooseDefaultTarget}
            />
            <span className="text-[11px] text-ink-3">
              {t("Default target for new installs. Pick it on a game to install there.")}
            </span>
          </div>
        )}
        {devices.isError ? (
          <p className="border border-danger/40 bg-danger/5 px-4 py-3 text-[12.5px] text-ink-2">
            {errorMessage(devices.error)}
          </p>
        ) : devices.isPending ? (
          <div className="h-20 animate-pulse border border-line bg-surface/40" />
        ) : deviceList.length === 0 ? (
          <div className="border border-line bg-surface/30 px-4 py-5 text-[12.5px] leading-relaxed text-ink-2">
            <p className="font-medium text-ink">{t("No PC is registered yet")}</p>
            <p className="mt-1">{t("To install on a PC, all three are required:")}</p>
            <ol className="mt-2 list-decimal space-y-1 ps-5">
              <li>{t("Ludex is open on the PC (the tray counts) and signed in.")}</li>
              <li>{t("The PC uses the same server URL as this phone.")}</li>
              <li>{t("Both devices use the same account.")}</li>
            </ol>
            <p className="mt-2">
              {t("Check the server on the PC:")}{" "}
              <Link to="/settings" className="underline underline-offset-4 hover:text-ink">
                {t("Settings → Servers")}
              </Link>
              . {t("Until then, requests stay in the account-wide queue.")}
            </p>
            {USING_MOCKS && <p className="mt-2 text-signal-text">{t("Demo data: the demo PC is simulated in the browser.")}</p>}
          </div>
        ) : (
          <ul className="grid gap-3 sm:grid-cols-2">
            {deviceList.map((device) => (
              <DeviceCard key={device.id} device={device} />
            ))}
          </ul>
        )}
      </section>

      {isError ? (
        <div className="mt-6 flex flex-wrap items-center gap-3 border border-danger/40 bg-danger/5 p-4 text-[13px] text-ink-2">
          <AlertTriangle className="size-4 text-danger" aria-hidden="true" />
          <span>{errorMessage(error)}</span>
          <Button variant="ghost" size="sm" onClick={() => void refetch()}>
            {t("Try again")}
          </Button>
        </div>
      ) : isPending ? (
        <div className="mt-6 space-y-3">
          {[0, 1].map((index) => (
            <div key={index} className="h-24 animate-pulse border border-line bg-surface/40" />
          ))}
        </div>
      ) : installed.length === 0 ? (
        <div className="mt-8">
          <EmptyState
            title={t("Nothing queued yet")}
            description={t("Open a game and choose “Install on the desktop” to queue it from this phone.")}
          >
            <Link to="/library">
              <Button variant="default" icon={<Download className="size-4" />}>
                {t("Browse the library")}
              </Button>
            </Link>
          </EmptyState>
        </div>
      ) : (
        <>
          <section className="mt-8" aria-labelledby="remote-active">
            <SectionHeader
              id="remote-active"
              title={t("In progress")}
              count={active.length}
              description={t("These requests wait for a desktop or are downloading on it.")}
            />
            {active.length === 0 ? (
              <p className="border border-line px-4 py-6 text-[13px] text-ink-3">
                {t("No install is running right now.")}
              </p>
            ) : (
              <ul className="flex flex-col gap-3">
                {active.map((install) => (
                  <RemoteInstallCard
                    key={install.id}
                    install={install}
                    targetDevice={deviceList.find((device) => device.id === install.targetDeviceId)}
                    cancelling={cancel.isPending && cancel.variables?.id === install.id}
                    onCancel={() => cancel.mutate(install)}
                  />
                ))}
              </ul>
            )}
          </section>

          {history.length > 0 && (
            <section className="mt-8" aria-labelledby="remote-history">
              <SectionHeader
                id="remote-history"
                title={t("Finished")}
                count={history.length}
                description={t("Ready games can be played on the desktop. Clearing a row never uninstalls it.")}
              />
              <ul className="flex flex-col gap-3">
                {history.map((install) => (
                  <RemoteInstallCard
                    key={install.id}
                    install={install}
                    targetDevice={deviceList.find((device) => device.id === install.targetDeviceId)}
                    cancelling={cancel.isPending && cancel.variables?.id === install.id}
                    onCancel={() => cancel.mutate(install)}
                  />
                ))}
              </ul>
            </section>
          )}
        </>
      )}

      <details className="mt-10 border border-line bg-surface/30 p-4 text-[12.5px] leading-relaxed text-ink-3">
        <summary className="cursor-pointer select-none text-[13px] font-medium text-ink-2">
          {t("Reaching the server from outside your network")}
        </summary>
        <div className="mt-3 space-y-2">
          <p>
            {t("Prefer a VPN (WireGuard, Tailscale) or your own HTTPS reverse proxy. Ludex authenticates every remote install request; a tunnel only carries bytes and is never treated as a login.")}
          </p>
          <p>
            {t("If you use a public tunnel such as OpenTunnel, anyone with the URL can reach the sign-in page. Keep two-factor credentials on the account, keep the tunnel URL private and revoke the tunnel when you are done. Ludex does not bundle or enable a tunnel by default.")}
          </p>
          {USING_MOCKS && <p className="text-signal-text">{t("Demo data: requests are simulated in the browser.")}</p>}
        </div>
      </details>
    </div>
  );
}

function DeviceCard({ device }: { device: Device }) {
  return (
    <li className="flex items-start gap-3 border border-line bg-surface/40 p-3">
      <Monitor className="mt-0.5 size-4 shrink-0 text-signal" aria-hidden="true" />
      <div className="min-w-0 flex-1">
        <div className="flex items-start justify-between gap-2">
          <p className="truncate text-[14px] font-medium text-ink">{device.name}</p>
          <Badge tone={device.online ? "ok" : "outline"}>
            {device.online ? t("Online") : t("Offline")}
          </Badge>
        </div>
        <p className="mt-0.5 text-[11.5px] text-ink-3">{deviceSummary(device)}</p>
        <p className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-[11.5px] text-ink-3">
          <span className="inline-flex items-center gap-1">
            {device.online ? (
              <Wifi className="size-3 text-ok" aria-hidden="true" />
            ) : (
              <WifiOff className="size-3" aria-hidden="true" />
            )}
            {device.online
              ? t("Online now")
              : t("Last seen {when}", { when: formatRelative(device.lastSeenAt) })}
          </span>
          <span>{t("{count} games installed", { count: device.installedCount })}</span>
          {device.freeDiskBytes !== null && (
            <span className="num">{t("{size} free", { size: formatBytes(device.freeDiskBytes) })}</span>
          )}
        </p>
      </div>
    </li>
  );
}

function RemoteInstallCard({
  install,
  targetDevice,
  cancelling,
  onCancel,
}: {
  install: RemoteInstall;
  targetDevice: Device | undefined;
  cancelling: boolean;
  onCancel: () => void;
}) {
  const percent = remotePercent(install);
  const offline = remoteLooksOffline(install);
  const year = install.completedAt ?? install.requestedAt;

  return (
    <li className="flex items-stretch gap-3 border border-line bg-surface/40 p-3 sm:gap-4 sm:p-4">
      <div className="w-14 shrink-0 self-center sm:w-16">
        <GameCover title={install.gameTitle} coverUrl={install.coverUrl} index={install.gameId} year={null} />
      </div>
      <div className="flex min-w-0 flex-1 flex-col gap-1.5">
        <div className="flex flex-wrap items-start justify-between gap-x-3 gap-y-1">
          <div className="min-w-0">
            <Link
              to={`/games/${install.gameId}`}
              className="block truncate text-[15px] font-medium leading-tight text-ink hover:text-signal-text"
            >
              {install.gameTitle}
            </Link>
            <p className="mt-0.5 flex flex-wrap items-center gap-x-2 gap-y-1 text-[11.5px] text-ink-3">
              {install.version && <span className="num">v{install.version}</span>}
              {install.sizeBytes !== null && <span className="num">{formatBytes(install.sizeBytes)}</span>}
              <span>{formatRelative(year)}</span>
              {install.device && <span>{install.device}</span>}
              {install.targetDeviceName && <span>{install.targetDeviceName}</span>}
            </p>
          </div>
          <Badge tone={remoteStatusTone(install.status)}>{remoteStatusLabel(install.status)}</Badge>
        </div>

        {percent !== null && <Progress value={percent} label={t("Install progress")} className="h-1.5" />}
        {percent === null && isRemoteActive(install.status) && (
          <div className="h-1.5 overflow-hidden bg-line" aria-hidden="true">
            <div className="h-full w-1/3 animate-pulse bg-signal/70" />
          </div>
        )}

        <div className="flex flex-wrap items-center justify-between gap-2">
          <p className="text-[11.5px] text-ink-3">
            {install.status === "queued" && install.targetDeviceName
              ? targetDevice && !targetDevice.online
                ? t("Will start when {device} is online.", { device: install.targetDeviceName })
                : t("Reserved for {device}. It starts as soon as that PC claims it.", { device: install.targetDeviceName })
              : offline
                ? t("The desktop is not connected yet. It will start when the app opens.")
                : install.message
                  ? install.message
                  : install.status === "ready"
                    ? t("Installed on the desktop.")
                    : t("Waiting for the desktop to pick it up.")}
          </p>
          <div className="flex items-center gap-1">
            {isRemoteActive(install.status) && (
              <Button
                variant="ghost"
                size="sm"
                className="h-11 sm:h-8"
                icon={<Trash2 className="size-3.5" />}
                loading={cancelling}
                onClick={onCancel}
              >
                {t("Cancel")}
              </Button>
            )}
            {!isRemoteActive(install.status) && (
              <Button
                variant="ghost"
                size="sm"
                className="h-11 sm:h-8"
                icon={<Trash2 className="size-3.5" />}
                loading={cancelling}
                onClick={onCancel}
              >
                {t("Clear")}
              </Button>
            )}
          </div>
        </div>
      </div>
    </li>
  );
}
