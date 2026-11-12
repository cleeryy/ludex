import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  AlertTriangle,
  CheckCircle2,
  Copy,
  Globe2,
  Loader2,
  RefreshCw,
  ShieldAlert,
  ShieldOff,
  Smartphone,
  Wifi,
} from "lucide-react";
import { isNative, native } from "@/lib/native";
import {
  INITIAL_REMOTE_PLAY_WIZARD,
  minutesUntilExpiry,
  moonlightAddress,
  portSummary,
  remotePlayFailure,
  remotePlayWizard,
  type RemotePlayStep,
  type RemotePlayWizard,
  type RemotePlayWizardEvent,
} from "@/lib/remote-play";
import type {
  RemotePlayCleanup,
  RemotePlayPortPlan,
  RemotePlayPortResult,
  RemotePlayReport,
  RemotePlayStatus,
  RemotePlayVerification,
} from "@/lib/types";
import { Badge, Button, Checkbox, Skeleton } from "@/components/ui/controls";
import { Dialog } from "@/components/ui/dialog";
import { t } from "@/lib/i18n";

const LEASE_OPTIONS = [1800, 3600, 7200];
const REFRESH_INTERVAL_MS = 5 * 60 * 1000;

function portPurpose(protocol: "tcp" | "udp", port: number): string {
  if (protocol === "tcp" && port === 47984) return t("HTTPS / pairing (NVHTTP)");
  if (protocol === "tcp" && port === 47989) return t("HTTP / pairing");
  if (protocol === "tcp" && port === 48010) return t("RTSP control");
  if (protocol === "udp" && port === 47998) return t("Video stream");
  if (protocol === "udp" && port === 47999) return t("Input control");
  if (protocol === "udp" && port === 48000) return t("Audio stream");
  if (protocol === "udp" && port === 48002) return t("Microphone (optional)");
  return t("Moonlight");
}

/**
 * Settings → Streaming → Play away from home (#179).
 *
 * VPN-first remote play, with an advanced UPnP mode that lists the exact
 * ports, maps them with a short lease, verifies the mapping and removes it on
 * exit. Sunshine's Web UI (47990) and mDNS (5353) are never mapped.
 */
export function RemotePlayPanel() {
  const [open, setOpen] = useState(false);
  const [wizard, setWizard] = useState<RemotePlayWizard>(INITIAL_REMOTE_PLAY_WIZARD);
  const [leaseSecs, setLeaseSecs] = useState(3600);
  const [microphone, setMicrophone] = useState(false);
  const [plan, setPlan] = useState<RemotePlayPortPlan | null>(null);
  const [consent, setConsent] = useState(false);
  const [report, setReport] = useState<RemotePlayReport | null>(null);
  const [verification, setVerification] = useState<RemotePlayVerification | null>(null);
  const [cleanup, setCleanup] = useState<RemotePlayCleanup | null>(null);
  const [busy, setBusy] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [probePublic, setProbePublic] = useState(false);

  const statusQuery = useQuery({
    queryKey: ["remote-play-status", probePublic],
    queryFn: () => native.remotePlayStatus({ probePublic }),
    enabled: isNative(),
    retry: false,
    staleTime: 15_000,
  });
  const refetchStatus = statusQuery.refetch;
  const autoAdvanced = useRef(false);
  const status = statusQuery.data ?? null;
  const activeCount = status?.active.mappings.length ?? 0;

  const dispatch = (event: RemotePlayWizardEvent) => setWizard((current) => remotePlayWizard(current, event));

  // Detection is asynchronous: stay on the detect step until the query settles,
  // then either resume an active session or show the mode choice.
  useEffect(() => {
    if (!open || !status || statusQuery.isFetching) return;
    if (wizard.step !== "detect") {
      autoAdvanced.current = true;
      return;
    }
    if (autoAdvanced.current) return;
    autoAdvanced.current = true;
    dispatch({ type: activeCount > 0 ? "resumed" : "detected" });
  }, [open, status, statusQuery.isFetching, wizard.step, activeCount]);

  // The consent screen always shows the backend's authoritative plan.
  useEffect(() => {
    if (wizard.step !== "consent") return;
    let cancelled = false;
    native
      .remotePlayPlan({ microphone, leaseSecs })
      .then((value) => {
        if (!cancelled) setPlan(value);
      })
      .catch((error) => {
        if (!cancelled) setActionError(remotePlayFailure(error).message);
      });
    return () => {
      cancelled = true;
    };
  }, [wizard.step, microphone, leaseSecs]);

  // Leaving Settings while the wizard is open is also an exit: remove any
  // mapping instead of leaving ports open until the lease expires.
  useEffect(
    () => () => {
      if (isNative()) void native.remotePlayDisable().catch(() => undefined);
    },
    [],
  );

  // Refresh the lease before it expires while the wizard stays open.
  useEffect(() => {
    if (!open || activeCount === 0) return;
    const timer = window.setInterval(() => {
      void native
        .remotePlayRefresh()
        .then(() => refetchStatus())
        .catch(() => undefined);
    }, REFRESH_INTERVAL_MS);
    return () => window.clearInterval(timer);
  }, [open, activeCount, refetchStatus]);

  const resetLocalState = () => {
    setWizard(INITIAL_REMOTE_PLAY_WIZARD);
    setPlan(null);
    setConsent(false);
    setReport(null);
    setVerification(null);
    setCleanup(null);
    setActionError(null);
    setCopied(false);
  };

  const openWizard = () => {
    resetLocalState();
    autoAdvanced.current = false;
    setWizard(
      activeCount > 0
        ? { ...INITIAL_REMOTE_PLAY_WIZARD, step: "test" }
        : INITIAL_REMOTE_PLAY_WIZARD,
    );
    setOpen(true);
    // Only the wizard asks a public address service what the internet sees.
    setProbePublic(true);
    void refetchStatus();
  };

  const failure = (error: unknown) => {
    const parsed = remotePlayFailure(error);
    return parsed.message || t("The remote play step failed.");
  };

  const enable = async () => {
    if (!consent) return;
    setBusy(true);
    setActionError(null);
    dispatch({ type: "consent" });
    try {
      const result = await native.remotePlayEnable({ microphone, leaseSecs, confirm: true });
      setReport(result);
      setVerification(result.verification);
      dispatch({ type: "mapped" });
      await refetchStatus();
    } catch (error) {
      dispatch({ type: "failed", message: failure(error) });
    } finally {
      setBusy(false);
    }
  };

  const verify = async () => {
    setBusy(true);
    setActionError(null);
    try {
      const result = await native.remotePlayVerify();
      setVerification(result);
      await refetchStatus();
    } catch (error) {
      setActionError(failure(error));
    } finally {
      setBusy(false);
    }
  };

  const disable = async (): Promise<RemotePlayCleanup | null> => {
    setBusy(true);
    setActionError(null);
    try {
      const result = await native.remotePlayDisable();
      setCleanup(result);
      setReport(null);
      setVerification(null);
      await refetchStatus();
      return result;
    } catch (error) {
      setActionError(failure(error));
      return null;
    } finally {
      setBusy(false);
    }
  };

  const closeWizard = () => {
    setOpen(false);
    // Closing the wizard is a cancel: always ask for a cleanup, even when the
    // status still says "off" or a previous attempt left a partial mapping.
    void disable();
  };

  const handleOpenChange = (next: boolean) => {
    if (next) {
      setOpen(true);
      return;
    }
    closeWizard();
  };

  const address = status ? moonlightAddress(status) : null;
  const directMapped = activeCount > 0;
  const directMode = wizard.mode === "upnp" || (wizard.mode === null && directMapped);

  const copyAddress = async () => {
    if (!address) return;
    try {
      await navigator.clipboard.writeText(address);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 2000);
    } catch {
      setCopied(false);
    }
  };

  const footer = (() => {
    if (wizard.step === "detect") {
      return (
        <div className="flex justify-end gap-2">
          <Button variant="ghost" disabled={busy} onClick={closeWizard}>
            {t("Close")}
          </Button>
          <Button
            variant="primary"
            icon={<RefreshCw className="size-3.5" />}
            loading={statusQuery.isFetching}
            onClick={() => void refetchStatus()}
          >
            {t("Detect again")}
          </Button>
        </div>
      );
    }
    if (wizard.step === "mode") {
      return (
        <div className="flex justify-between gap-2">
          <Button variant="ghost" onClick={() => dispatch({ type: "back" })}>
            {t("Back")}
          </Button>
          <Button variant="ghost" disabled={busy} onClick={closeWizard}>
            {t("Close")}
          </Button>
        </div>
      );
    }
    if (wizard.step === "consent") {
      return (
        <div className="flex justify-between gap-2">
          <Button variant="ghost" onClick={() => dispatch({ type: "back" })}>
            {t("Back")}
          </Button>
          <Button variant="primary" disabled={!consent || !plan} loading={busy} onClick={() => void enable()}>
            {t("Map these ports")}
          </Button>
        </div>
      );
    }
    if (wizard.step === "map") {
      return (
        <div className="flex justify-between gap-2">
          <Button variant="ghost" onClick={() => dispatch({ type: "back" })}>
            {t("Back")}
          </Button>
          <div className="flex gap-2">
            <Button variant="ghost" disabled={busy} onClick={closeWizard}>
              {t("Cancel and clean up")}
            </Button>
            <Button variant="primary" loading={busy} onClick={() => void enable()}>
              {t("Retry mapping")}
            </Button>
          </div>
        </div>
      );
    }
    if (wizard.step === "verify") {
      return (
        <div className="flex flex-wrap justify-between gap-2">
          <Button variant="ghost" onClick={() => dispatch({ type: "back" })}>
            {t("Back")}
          </Button>
          <div className="flex flex-wrap gap-2">
            <Button variant="ghost" icon={<ShieldOff className="size-3.5" />} loading={busy} onClick={() => void disable()}>
              {t("Remove mapping")}
            </Button>
            <Button variant="default" icon={<RefreshCw className="size-3.5" />} loading={busy} onClick={() => void verify()}>
              {t("Verify again")}
            </Button>
            <Button
              variant="primary"
              disabled={!verification?.verified}
              onClick={() => dispatch({ type: "verified" })}
            >
              {t("Test with Moonlight")}
            </Button>
          </div>
        </div>
      );
    }
    return (
      <div className="flex flex-wrap justify-between gap-2">
        {directMapped ? (
          <Button
            variant="danger"
            icon={<ShieldOff className="size-3.5" />}
            loading={busy}
            onClick={() => void disable()}
          >
            {t("Disable and remove mappings")}
          </Button>
        ) : (
          <Button variant="ghost" onClick={() => dispatch({ type: "back" })}>
            {t("Back")}
          </Button>
        )}
        <Button variant="default" onClick={closeWizard}>
          {t("Done")}
        </Button>
      </div>
    );
  })();

  return (
    <div className="mt-8 border border-line p-5">
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div>
          <div className="flex items-center gap-2">
            <Globe2 className="size-4 text-ink-3" aria-hidden="true" />
            <h4 className="text-[13.5px] font-semibold text-ink">{t("Play away from home")}</h4>
            <Badge tone={directMapped ? "ok" : "outline"}>
              {directMapped ? t("direct mode on") : t("off")}
            </Badge>
          </div>
          <p className="mt-1 max-w-[76ch] text-[12px] leading-relaxed text-ink-3">
            {t(
              "Play Moonlight on your phone from outside the house. A VPN or mesh network is the recommended path; direct mode forwards only the Sunshine ports, with a short lease and an automatic cleanup.",
            )}
          </p>
        </div>
        <Button variant="default" icon={<Wifi className="size-3.5" />} onClick={openWizard} disabled={!isNative()}>
          {t("Set up remote play")}
        </Button>
      </div>

      <dl className="mt-4 grid gap-x-6 gap-y-2 sm:grid-cols-2 xl:grid-cols-3">
        <div className="flex items-baseline justify-between gap-3 border-b border-line py-1.5">
          <dt className="text-[11.5px] text-ink-3">{t("LAN address")}</dt>
          <dd className="font-mono text-[10.5px] text-ink-2">{status?.lanIp ?? t("detecting…")}</dd>
        </div>
        <div className="flex items-baseline justify-between gap-3 border-b border-line py-1.5">
          <dt className="text-[11.5px] text-ink-3">{t("VPN")}</dt>
          <dd className="truncate text-[11.5px] text-ink-2">
            {status?.vpn.detected ? status.vpn.services.join(", ") || t("detected") : t("not detected")}
          </dd>
        </div>
        <div className="flex items-baseline justify-between gap-3 border-b border-line py-1.5">
          <dt className="text-[11.5px] text-ink-3">{t("Router UPnP")}</dt>
          <dd className="truncate text-[11.5px] text-ink-2">
            {status?.upnp.available
              ? status.upnp.friendlyName || status.upnp.model || t("available")
              : t("not available")}
          </dd>
        </div>
      </dl>

      <Dialog
        open={open}
        onOpenChange={handleOpenChange}
        title={t("Play away from home")}
        description={t(
          "VPN first. Direct mode is advanced: it exposes the Sunshine ports to the internet and is removed when you close this wizard.",
        )}
        size="lg"
        dismissible={!busy}
        footer={footer}
      >
        <WizardProgress step={wizard.step} />

        {actionError && (
          <p role="alert" className="mt-3 border-l-2 border-danger pl-3 text-[12.5px] leading-relaxed text-danger">
            {actionError}
          </p>
        )}

        {wizard.step === "detect" && (
          <DetectStep status={status} loading={statusQuery.isPending} error={statusQuery.isError} />
        )}

        {wizard.step === "mode" && status && (
          <ModeStep
            status={status}
            onChoose={(mode) => {
              dispatch({ type: "choose", mode });
              if (mode !== "upnp") void refetchStatus();
            }}
          />
        )}

        {wizard.step === "consent" && (
          <ConsentStep
            plan={plan}
            leaseSecs={leaseSecs}
            onLease={setLeaseSecs}
            microphone={microphone}
            onMicrophone={setMicrophone}
            consent={consent}
            onConsent={setConsent}
            status={status}
            error={wizard.error}
          />
        )}

        {wizard.step === "map" && (
          <div className="mt-6 flex flex-col items-center gap-3 py-8 text-center">
            <Loader2 className={busy ? "size-6 animate-spin text-signal" : "size-6 text-ink-3"} aria-hidden="true" />
            <p className="max-w-[46ch] text-[12.5px] leading-relaxed text-ink-2">
              {busy
                ? t("Asking the router for the mappings and verifying each port…")
                : t("The mapping did not complete. Nothing is left behind: retry, or cancel and clean up.")}
            </p>
            {wizard.error && <p className="max-w-[60ch] text-[12px] leading-relaxed text-danger">{wizard.error}</p>}
          </div>
        )}

        {wizard.step === "verify" && (
          <VerifyStep verification={verification} report={report} busy={busy} />
        )}

        {wizard.step === "test" && !status && (
          <div className="mt-4 flex flex-col gap-2">
            <Skeleton className="h-16 w-full" />
            <Skeleton className="h-24 w-full" />
          </div>
        )}

        {wizard.step === "test" && status && (
          <TestStep
            status={status}
            direct={directMode}
            address={address}
            copied={copied}
            onCopy={() => void copyAddress()}
            cleanup={cleanup}
          />
        )}
      </Dialog>
    </div>
  );
}

function WizardProgress({ step }: { step: RemotePlayStep }) {
  const steps: Array<{ id: RemotePlayStep; label: string }> = [
    { id: "detect", label: t("Detect") },
    { id: "mode", label: t("Mode") },
    { id: "consent", label: t("Consent") },
    { id: "map", label: t("Map") },
    { id: "verify", label: t("Verify") },
    { id: "test", label: t("Moonlight") },
  ];
  const current = steps.findIndex((item) => item.id === step);
  return (
    <ol className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-line pb-3 text-[10.5px] uppercase tracking-[0.08em]">
      {steps.map((item, index) => (
        <li
          key={item.id}
          className={
            index === current
              ? "font-semibold text-ink"
              : index < current
                ? "text-ink-3"
                : "text-ink-3/50"
          }
          aria-current={index === current ? "step" : undefined}
        >
          {index + 1}. {item.label}
        </li>
      ))}
    </ol>
  );
}

function DetectStep({
  status,
  loading,
  error,
}: {
  status: RemotePlayStatus | null;
  loading: boolean;
  error: boolean;
}) {
  if (loading) {
    return (
      <div className="mt-4 flex flex-col gap-2">
        <Skeleton className="h-5 w-2/3" />
        <Skeleton className="h-16 w-full" />
        <Skeleton className="h-16 w-full" />
      </div>
    );
  }
  if (error || !status) {
    return (
      <p role="alert" className="mt-4 border-l-2 border-danger pl-3 text-[12.5px] leading-relaxed text-danger">
        {t("The network detection failed. Check the connection and detect again.")}
      </p>
    );
  }
  return (
    <div className="mt-4 flex flex-col gap-3">
      <div className="grid gap-2 sm:grid-cols-2">
        <StatusRow
          label={t("LAN address")}
          value={status.lanIp ?? t("not found")}
          ok={Boolean(status.lanIp)}
        />
        <StatusRow
          label={t("Public address")}
          value={status.publicIp ?? t("not available")}
          ok={Boolean(status.publicIp)}
        />
        <StatusRow
          label={t("Sunshine")}
          value={status.sunshine.running ? t("running") : t("not running")}
          ok={status.sunshine.running}
        />
        <StatusRow
          label={t("Sunshine ports")}
          value={status.sunshine.portsOpen.join(", ") || t("not probed")}
          ok={status.sunshine.portsOpen.length > 0}
        />
        <StatusRow
          label={t("Router UPnP")}
          value={status.upnp.available ? status.upnp.friendlyName || t("available") : t("not available")}
          ok={status.upnp.available}
        />
        <StatusRow
          label={t("IPv6")}
          value={status.ipv6 ?? t("not available")}
          ok={Boolean(status.ipv6)}
        />
        <StatusRow
          label={t("VPN")}
          value={status.vpn.detected ? status.vpn.services.join(", ") || t("detected") : t("not detected")}
          ok={status.vpn.detected}
        />
      </div>

      {status.sunshine.blockers.length > 0 && (
        <WarningList title={t("Sunshine must be fixed before any port is mapped")} items={status.sunshine.blockers} tone="danger" />
      )}
      {status.warnings.length > 0 && <WarningList title={t("Detection notes")} items={status.warnings} />}

      <p className="text-[12px] leading-relaxed text-ink-2">{status.message}</p>
      {!status.direct.allowed && (
        <p className="border-l-2 border-warn pl-3 text-[12px] leading-relaxed text-ink-2">
          <span className="font-medium text-ink">{t("Direct mode is unavailable:")} </span>
          {status.direct.message}
        </p>
      )}
    </div>
  );
}

function StatusRow({ label, value, ok }: { label: string; value: string; ok: boolean }) {
  return (
    <div className="flex items-baseline justify-between gap-3 border-b border-line py-1.5">
      <span className="text-[11.5px] text-ink-3">{label}</span>
      <span className={`truncate font-mono text-[10.5px] ${ok ? "text-ink-2" : "text-warn"}`}>{value}</span>
    </div>
  );
}

function ModeStep({
  status,
  onChoose,
}: {
  status: RemotePlayStatus;
  onChoose: (mode: "vpn" | "lan" | "upnp") => void;
}) {
  return (
    <div className="mt-4 flex flex-col gap-3">
      <button
        type="button"
        onClick={() => onChoose("vpn")}
        className="border border-line p-3 text-left transition-colors hover:border-signal/60 hover:bg-surface-2"
      >
        <span className="flex items-center gap-2 text-[12.5px] font-medium text-ink">
          <Wifi className="size-3.5 text-ok" aria-hidden="true" />
          {t("VPN or mesh (recommended)")}
          <Badge tone="ok">{t("recommended")}</Badge>
        </span>
        <span className="mt-1 block text-[11.5px] leading-relaxed text-ink-3">
          {t(
            "Tailscale, WireGuard, ZeroTier, Netbird or Headscale reaches this machine without opening a port. Nothing is exposed to the internet.",
          )}
        </span>
        {status.vpn.detected && (
          <span className="mt-1 block font-mono text-[10.5px] text-ok">
            {t("Detected:")} {status.vpn.services.join(", ") || status.vpn.address}
          </span>
        )}
      </button>

      <button
        type="button"
        onClick={() => onChoose("lan")}
        className="border border-line p-3 text-left transition-colors hover:border-signal/60 hover:bg-surface-2"
      >
        <span className="flex items-center gap-2 text-[12.5px] font-medium text-ink">
          <Smartphone className="size-3.5 text-ink-3" aria-hidden="true" />
          {t("Same Wi-Fi (LAN)")}
        </span>
        <span className="mt-1 block text-[11.5px] leading-relaxed text-ink-3">
          {t("Enter the LAN address in Moonlight. This works at home only and opens no port.")}
        </span>
      </button>

      <button
        type="button"
        onClick={() => onChoose("upnp")}
        disabled={!status.direct.allowed}
        className="border border-line p-3 text-left transition-colors hover:border-warn/60 hover:bg-surface-2 disabled:cursor-not-allowed disabled:opacity-50"
      >
        <span className="flex items-center gap-2 text-[12.5px] font-medium text-ink">
          <Globe2 className="size-3.5 text-warn" aria-hidden="true" />
          {t("Direct mode with UPnP (advanced)")}
        </span>
        <span className="mt-1 block text-[11.5px] leading-relaxed text-ink-3">
          {status.direct.allowed
            ? t("Forward the exact Moonlight ports through the router with a short lease. The host is exposed to the internet while it is on.")
            : status.direct.message}
        </span>
      </button>

      <p className="text-[11.5px] leading-relaxed text-ink-3">{status.vpn.message}</p>
    </div>
  );
}

function ConsentStep({
  plan,
  leaseSecs,
  onLease,
  microphone,
  onMicrophone,
  consent,
  onConsent,
  status,
  error,
}: {
  plan: RemotePlayPortPlan | null;
  leaseSecs: number;
  onLease: (value: number) => void;
  microphone: boolean;
  onMicrophone: (value: boolean) => void;
  consent: boolean;
  onConsent: (value: boolean) => void;
  status: RemotePlayStatus | null;
  error: string | null;
}) {
  return (
    <div className="mt-4 flex flex-col gap-4">
      <div className="border border-warn/50 bg-warn/5 p-3">
        <p className="flex items-start gap-2 text-[12px] leading-relaxed text-ink-2">
          <ShieldAlert className="mt-0.5 size-3.5 shrink-0 text-warn" aria-hidden="true" />
          <span>
            {t(
              "Direct mode exposes this machine to the internet. Sunshine's PIN and TLS are the only authentication. Anyone who finds the ports can attempt a pairing request.",
            )}
          </span>
        </p>
      </div>

      {!plan && <Skeleton className="h-32 w-full" />}

      {plan && (
        <>
          <div className="overflow-x-auto border border-line">
            <table className="w-full text-left text-[11.5px]">
              <thead className="bg-surface-2 text-[10px] uppercase tracking-[0.08em] text-ink-3">
                <tr>
                  <th className="px-2 py-1.5 font-medium">{t("Protocol")}</th>
                  <th className="px-2 py-1.5 font-medium">{t("Port")}</th>
                  <th className="px-2 py-1.5 font-medium">{t("Purpose")}</th>
                </tr>
              </thead>
              <tbody>
                {[...plan.tcp.map((port) => ["tcp", port] as const), ...plan.udp.map((port) => ["udp", port] as const)].map(
                  ([protocol, port]) => (
                    <tr key={`${protocol}-${port}`} className="border-t border-line">
                      <td className="px-2 py-1 font-mono uppercase text-ink-3">{protocol}</td>
                      <td className="px-2 py-1 font-mono text-ink">{port}</td>
                      <td className="px-2 py-1 text-ink-3">{portPurpose(protocol, port)}</td>
                    </tr>
                  ),
                )}
              </tbody>
            </table>
          </div>

          <div className="flex flex-wrap items-end justify-between gap-3">
            <label className="flex flex-col gap-1">
              <span className="text-[11.5px] font-medium text-ink">{t("Lease")}</span>
              <select
                value={leaseSecs}
                onChange={(event) => onLease(Number(event.target.value))}
                className="h-8 border border-line-2 bg-surface px-2 text-[12px] text-ink outline-none focus:border-signal"
              >
                {LEASE_OPTIONS.filter(
                  (value) => value >= (status?.leaseMinSecs ?? 600) && value <= (status?.leaseMaxSecs ?? 7200),
                ).map((value) => (
                  <option key={value} value={value}>
                    {t("{count} minutes", { count: value / 60 })}
                  </option>
                ))}
              </select>
            </label>
            <label className="flex items-center gap-2 text-[11.5px] text-ink-2">
              <Checkbox
                checked={microphone}
                onCheckedChange={onMicrophone}
                label={t("Also forward the microphone port")}
              />
              {t("Also forward the microphone port (UDP 48002)")}
            </label>
          </div>

          <div className="flex items-start gap-2 border-t border-line pt-3">
            <Checkbox
              checked={consent}
              onCheckedChange={onConsent}
              label={t("I understand the host is exposed to the internet")}
            />
            <span className="text-[11.5px] leading-relaxed text-ink-2">
              {t(
                "I understand the exact ports above are exposed to the internet until I disable direct mode, and that the mapping is removed automatically when this wizard closes.",
              )}
            </span>
          </div>

          <p className="font-mono text-[10.5px] text-ink-3">{portSummary(plan)}</p>

          <WarningList title={t("Exposure and caveats")} items={plan.warnings} />
          <WarningList
            title={t("Never forwarded")}
            items={[
              t("TCP 47990 (Sunshine Web UI and pairing approval) stays on the LAN."),
              t("UDP 5353 (mDNS) stays on the LAN."),
            ]}
          />
          {error && <p className="text-[12px] leading-relaxed text-danger">{error}</p>}
        </>
      )}
    </div>
  );
}

function VerifyStep({
  verification,
  report,
  busy,
}: {
  verification: RemotePlayVerification | null;
  report: RemotePlayReport | null;
  busy: boolean;
}) {
  return (
    <div className="mt-4 flex flex-col gap-3">
      <p className="text-[12px] leading-relaxed text-ink-2">
        {busy ? t("Checking every mapped port…") : verification?.message ?? t("No verification ran yet.")}
      </p>
      {verification && (
        <>
          <div className="overflow-x-auto border border-line">
            <table className="w-full text-left text-[11.5px]">
              <thead className="bg-surface-2 text-[10px] uppercase tracking-[0.08em] text-ink-3">
                <tr>
                  <th className="px-2 py-1.5 font-medium">{t("Port")}</th>
                  <th className="px-2 py-1.5 font-medium">{t("Mapping")}</th>
                  <th className="px-2 py-1.5 font-medium">{t("Reachability")}</th>
                  <th className="px-2 py-1.5 font-medium">{t("Detail")}</th>
                </tr>
              </thead>
              <tbody>
                {verification.ports.map((port) => (
                  <PortRow key={`${port.protocol}-${port.port}`} port={port} />
                ))}
              </tbody>
            </table>
          </div>
          {verification.notes.length > 0 && <WarningList title={t("Reachability notes")} items={verification.notes} />}
          {report && (
            <dl className="grid gap-x-6 gap-y-1 sm:grid-cols-2">
              <div className="flex justify-between gap-3 border-b border-line py-1.5">
                <dt className="text-[11.5px] text-ink-3">{t("Public address")}</dt>
                <dd className="font-mono text-[10.5px] text-ink-2">{report.publicIp ?? t("unknown")}</dd>
              </div>
              <div className="flex justify-between gap-3 border-b border-line py-1.5">
                <dt className="text-[11.5px] text-ink-3">{t("Lease")}</dt>
                <dd className="text-[11.5px] text-ink-2">{t("{count} minutes", { count: Math.round(report.leaseSecs / 60) })}</dd>
              </div>
            </dl>
          )}
          <p className="text-[11.5px] leading-relaxed text-ink-3">
            {t(
              "A mapped port that this machine cannot reach is not necessarily closed: most routers block NAT loopback. The real test is Moonlight on mobile data.",
            )}
          </p>
        </>
      )}
    </div>
  );
}

function PortRow({ port }: { port: RemotePlayPortResult }) {
  const tone =
    port.status === "reachable" || port.status === "mapped"
      ? "text-ok"
      : port.status === "not_mapped" || port.status === "conflict"
        ? "text-danger"
        : "text-warn";
  return (
    <tr className="border-t border-line">
      <td className="px-2 py-1 font-mono text-ink">
        {port.protocol.toUpperCase()} {port.port}
      </td>
      <td className={`px-2 py-1 font-mono ${port.mapped ? "text-ok" : "text-danger"}`}>
        {port.mapped ? t("mapped") : t("not mapped")}
      </td>
      <td className={`px-2 py-1 font-mono ${tone}`}>
        {port.reachable === null ? t("not probed") : port.reachable ? t("reachable") : t("unreachable from here")}
      </td>
      <td className="px-2 py-1 text-ink-3">{port.note}</td>
    </tr>
  );
}

function TestStep({
  status,
  direct,
  address,
  copied,
  onCopy,
  cleanup,
}: {
  status: RemotePlayStatus;
  direct: boolean;
  address: string | null;
  copied: boolean;
  onCopy: () => void;
  cleanup: RemotePlayCleanup | null;
}) {
  const minutes = minutesUntilExpiry(status.active, Math.floor(Date.now() / 1000));
  return (
    <div className="mt-4 flex flex-col gap-4">
      <div className="border border-line p-3">
        <span className="text-[10px] uppercase tracking-[0.08em] text-ink-3">{t("Moonlight address")}</span>
        <div className="mt-1 flex flex-wrap items-center justify-between gap-2">
          <code className="font-mono text-[13px] text-ink">{address ?? t("not available")}</code>
          <Button
            variant="default"
            size="sm"
            icon={<Copy className="size-3.5" />}
            onClick={onCopy}
            disabled={!address}
          >
            {copied ? t("Copied") : t("Copy")}
          </Button>
        </div>
        {direct && minutes !== null && (
          <p className="mt-2 text-[11.5px] text-ink-3">
            {t("The mapping expires in about {count} minutes; it is refreshed while this wizard is open.", { count: minutes })}
          </p>
        )}
      </div>

      <ol className="flex flex-col gap-2 text-[12px] leading-relaxed text-ink-2">
        <li className="flex gap-2">
          <span className="font-mono text-ink-3">1.</span>
          {t("Install Moonlight on the phone or tablet you will play on.")}
        </li>
        <li className="flex gap-2">
          <span className="font-mono text-ink-3">2.</span>
          {t("Add a PC with the address above, then enter the PIN Sunshine shows on the host.")}
        </li>
        <li className="flex gap-2">
          <span className="font-mono text-ink-3">3.</span>
          {t("Start on Wi-Fi once to pair, then switch the phone to mobile data and start the stream again.")}
        </li>
        <li className="flex gap-2">
          <span className="font-mono text-ink-3">4.</span>
          {t("If pairing fails on mobile data, the mapping or the ISP is blocking it: close this wizard to clean up, and use a VPN instead.")}
        </li>
      </ol>

      {direct ? (
        <p className="border-l-2 border-warn pl-3 text-[11.5px] leading-relaxed text-ink-2">
          {t(
            "Direct mode is on. Closing this wizard or clicking Disable removes every mapping; the lease is the fallback if Ludex crashes.",
          )}
        </p>
      ) : (
        <p className="border-l-2 border-ok/60 pl-3 text-[11.5px] leading-relaxed text-ink-2">
          {status.vpn.detected
            ? status.vpn.message
            : t("No port is forwarded in this mode. The address above only works on the network it belongs to.")}
        </p>
      )}

      {cleanup && (
        <div className="border border-line p-3">
          <p className="flex items-center gap-2 text-[12px] font-medium text-ink">
            {cleanup.complete ? (
              <CheckCircle2 className="size-3.5 text-ok" aria-hidden="true" />
            ) : (
              <AlertTriangle className="size-3.5 text-warn" aria-hidden="true" />
            )}
            {cleanup.message}
          </p>
          {cleanup.ports.length > 0 && (
            <ul className="mt-2 grid gap-1 sm:grid-cols-2">
              {cleanup.ports.map((port) => (
                <li key={`${port.protocol}-${port.port}`} className="font-mono text-[10.5px] text-ink-3">
                  {port.protocol.toUpperCase()} {port.port} — {port.status}
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </div>
  );
}

function WarningList({
  title,
  items,
  tone = "warn",
}: {
  title: string;
  items: string[];
  tone?: "warn" | "danger";
}) {
  if (items.length === 0) return null;
  return (
    <div className={tone === "danger" ? "border-l-2 border-danger pl-3" : "border-l-2 border-warn/60 pl-3"}>
      <p className={`text-[11.5px] font-medium ${tone === "danger" ? "text-danger" : "text-ink-2"}`}>{title}</p>
      <ul className="mt-1 flex list-disc flex-col gap-0.5 pl-4 text-[11.5px] leading-relaxed text-ink-3">
        {items.map((item) => (
          <li key={item}>{item}</li>
        ))}
      </ul>
    </div>
  );
}
