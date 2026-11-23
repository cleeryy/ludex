import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  AlertTriangle,
  CheckCircle2,
  ExternalLink,
  KeyRound,
  Lock,
  MonitorPlay,
  RefreshCw,
  RotateCcw,
  ShieldCheck,
  Sparkles,
} from "lucide-react";
import { errorMessage } from "@/lib/http";
import { native } from "@/lib/native";
import { qk } from "@/lib/query-keys";
import {
  editSummary,
  packetSizeForMtu,
  portSummary,
  selfTestCopy,
  statusLabel,
  statusTone,
  streamCheckCopy,
} from "@/lib/stream-host";
import type {
  FirewallPlan,
  StreamConfigPlan,
  StreamHostReport,
  StreamPairingOutcome,
  StreamSelfTestReport,
} from "@/lib/types";
import { useToast } from "@/state/toasts";
import { Badge, Button, Input, Skeleton } from "@/components/ui/controls";
import { Dialog } from "@/components/ui/dialog";
import { t } from "@/lib/i18n";

/**
 * Settings → Streaming → "Stream to another device" (#291).
 *
 * Detection, the self-test, the config dry-run with its revert and the firewall
 * consent all live here. Nothing is written or opened without the user seeing
 * the exact plan first.
 */
export function StreamHostPanel() {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const [output, setOutput] = useState<string | null>(null);
  const [interfaceName, setInterfaceName] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [plan, setPlan] = useState<StreamConfigPlan | null>(null);
  const [selfTest, setSelfTest] = useState<StreamSelfTestReport | null>(null);
  const [firewallOpen, setFirewallOpen] = useState(false);
  const [pairPin, setPairPin] = useState("");
  const [pairUser, setPairUser] = useState("");
  const [pairPassword, setPairPassword] = useState("");
  const [pairRemember, setPairRemember] = useState(true);
  const [pairing, setPairing] = useState<StreamPairingOutcome | null>(null);

  const host = useQuery({
    queryKey: qk.streamHost.status,
    queryFn: () => native.streamHostStatus(),
    retry: false,
    staleTime: 15_000,
  });
  const firewall = useQuery({
    queryKey: qk.streamHost.firewall,
    queryFn: () => native.streamHostFirewallPlan(),
    retry: false,
    staleTime: 30_000,
  });

  const refresh = async () => {
    await queryClient.invalidateQueries({ queryKey: qk.streamHost.status });
    await queryClient.invalidateQueries({ queryKey: qk.streamHost.firewall });
    setSelfTest(null);
  };

  const runSelfTest = async () => {
    setBusy("self-test");
    try {
      setSelfTest(await native.streamHostSelfTest());
    } catch (cause) {
      push({ title: t("The self-test failed"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setBusy(null);
    }
  };

  const previewConfig = async () => {
    setBusy("plan");
    try {
      setPlan(await native.streamHostConfigPlan(output, interfaceName));
    } catch (cause) {
      push({ title: t("Could not build the plan"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setBusy(null);
    }
  };

  const applyConfig = async () => {
    setBusy("apply");
    try {
      const applied = await native.streamHostConfigApply({
        output,
        interfaceName,
        consent: true,
      });
      push({
        title: applied.written ? t("sunshine.conf updated") : t("Nothing was written"),
        description: applied.message,
        tone: applied.written ? "success" : "default",
      });
      setPlan(null);
      await refresh();
    } catch (cause) {
      push({ title: t("Could not write sunshine.conf"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setBusy(null);
    }
  };

  const revertConfig = async () => {
    setBusy("revert-config");
    try {
      const reverted = await native.streamHostConfigRevert();
      push({ title: t("sunshine.conf restored"), description: reverted.message, tone: "success" });
      await refresh();
    } catch (cause) {
      push({ title: t("Could not revert sunshine.conf"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setBusy(null);
    }
  };

  const applyFirewall = async () => {
    setBusy("firewall");
    try {
      const report = await native.streamHostFirewallApply(true);
      push({ title: t("Firewall updated"), description: report.message, tone: "success" });
      setFirewallOpen(false);
      await refresh();
    } catch (cause) {
      push({ title: t("Could not open the ports"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setBusy(null);
    }
  };

  const revertFirewall = async () => {
    setBusy("revert-firewall");
    try {
      const report = await native.streamHostFirewallRevert();
      push({ title: t("Firewall rules closed"), description: report.message, tone: "success" });
      await refresh();
    } catch (cause) {
      push({ title: t("Could not close the rules"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setBusy(null);
    }
  };

  const pair = async () => {
    setBusy("pair");
    try {
      const outcome = await native.streamHostPair({
        pin: pairPin,
        username: pairUser.trim() || null,
        password: pairPassword || null,
        remember: pairRemember,
      });
      setPairing(outcome);
      setPairPin("");
      if (outcome.mode === "paired") {
        push({ title: t("Paired"), description: outcome.message, tone: "success" });
        await refresh();
      } else if (outcome.mode === "credentials-required" || outcome.mode === "unreachable") {
        // The API cannot complete the pairing: open Sunshine's own page and
        // keep the reason visible instead of sending the user to a wiki.
        try {
          await native.streamHostPairingPage();
        } catch {
          // The outcome message already names the URL to visit.
        }
      }
    } catch (cause) {
      push({ title: t("Pairing failed"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setBusy(null);
    }
  };

  const openPairingPage = async () => {
    setBusy("page");
    try {
      const outcome = await native.streamHostPairingPage();
      setPairing(outcome);
    } catch (cause) {
      push({ title: t("Could not open the Sunshine page"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="mt-8 border border-line p-5">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="flex items-center gap-2">
          <MonitorPlay className="size-4 text-ink-3" aria-hidden="true" />
          <h4 className="text-body font-semibold text-ink">{t("Stream to another device")}</h4>
          {host.data && (
            <Badge tone={host.data.ready ? "ok" : "danger"}>
              {host.data.ready ? t("ready") : t("needs attention")}
            </Badge>
          )}
        </div>
        <Button
          size="sm"
          variant="ghost"
          icon={<RefreshCw className="size-3.5" />}
          loading={host.isFetching}
          onClick={() => void refresh()}
        >
          {t("Rescan")}
        </Button>
      </div>
      <p className="mt-1 max-w-[80ch] text-label leading-relaxed text-ink-3">
        {t("Checks Sunshine, the capture path and the encoder, then writes only the settings a Moonlight stream needs. Every change has a backup and a revert.")}
      </p>

      {host.isPending && (
        <div className="mt-4 flex flex-col gap-2">
          <Skeleton className="h-4 w-2/3" />
          <Skeleton className="h-4 w-1/2" />
        </div>
      )}
      {host.isError && (
        <p role="alert" className="mt-4 border-l-2 border-danger pl-3 text-label text-danger">
          {errorMessage(host.error)}
        </p>
      )}

      {host.data && (
        <>
          <p className="mt-3 text-label leading-relaxed text-ink-3">{host.data.message}</p>
          <DetectionTable report={host.data} />

          {host.data.warnings.length > 0 && (
            <ul className="mt-3 flex flex-col gap-1.5 text-meta leading-relaxed text-ink-3">
              {host.data.warnings.map((warning) => (
                <li key={warning} className="flex items-start gap-2 border-l-2 border-line-2 pl-2">
                  <AlertTriangle className="mt-0.5 size-3 shrink-0 text-warn" aria-hidden="true" />
                  <span>{warning}</span>
                </li>
              ))}
            </ul>
          )}

          <SelfTestStep
            result={selfTest}
            busy={busy === "self-test"}
            onRun={() => void runSelfTest()}
          />

          <ConfigStep
            report={host.data}
            output={output}
            interfaceName={interfaceName}
            busy={busy}
            onOutput={setOutput}
            onInterface={setInterfaceName}
            onPreview={() => void previewConfig()}
            onRevert={() => void revertConfig()}
          />

          <FirewallStep
            plan={firewall.data}
            busy={busy}
            onOpen={() => setFirewallOpen(true)}
            onRevert={() => void revertFirewall()}
          />

          <PairingStep
            stored={host.data.credentialsStored}
            pin={pairPin}
            username={pairUser}
            password={pairPassword}
            remember={pairRemember}
            busy={busy === "pair" || busy === "page"}
            outcome={pairing}
            onPin={setPairPin}
            onUsername={setPairUser}
            onPassword={setPairPassword}
            onRemember={setPairRemember}
            onPair={() => void pair()}
            onOpenPage={() => void openPairingPage()}
          />
        </>
      )}

      <Dialog
        open={plan !== null}
        onOpenChange={(open) => !open && setPlan(null)}
        title={t("Write sunshine.conf?")}
        description={t("Only output_name, stream_audio and packet_size change. Every other line stays untouched and a byte-identical backup is written first.")}
        footer={
          <div className="flex justify-end gap-2">
            <Button variant="ghost" onClick={() => setPlan(null)}>
              {t("Cancel")}
            </Button>
            <Button
              variant="primary"
              loading={busy === "apply"}
              disabled={!plan?.applicable}
              onClick={() => void applyConfig()}
            >
              {t("Write with backup")}
            </Button>
          </div>
        }
      >
        {plan && <ConfigPlanBody plan={plan} />}
      </Dialog>

      <Dialog
        open={firewallOpen}
        onOpenChange={setFirewallOpen}
        title={t("Open Sunshine's ports?")}
        description={t("Your desktop asks for administrator approval. Ludex opens these exact ranges and nothing else.")}
        footer={
          <div className="flex justify-end gap-2">
            <Button variant="ghost" onClick={() => setFirewallOpen(false)}>
              {t("Cancel")}
            </Button>
            <Button
              variant="primary"
              loading={busy === "firewall"}
              disabled={!firewall.data?.supported || Boolean(firewall.data?.blocked)}
              onClick={() => void applyFirewall()}
            >
              {t("Open these ports")}
            </Button>
          </div>
        }
      >
        {firewall.data && <FirewallBody plan={firewall.data} />}
      </Dialog>
    </div>
  );
}

function DetectionTable({ report }: { report: StreamHostReport }) {
  return (
    <dl className="mt-4 border-t border-line">
      {report.checks.map((check) => (
        <div key={check.id} className="flex items-start justify-between gap-4 border-b border-line py-2">
          <dt className="flex min-w-0 items-start gap-2">
            <Badge tone={statusTone(check.status)}>{statusLabel(check.status)}</Badge>
            <span className="text-label text-ink">{streamCheckCopy(check.id)}</span>
          </dt>
          <dd className="min-w-0 max-w-[60%] text-right">
            <p className="break-words font-mono text-micro text-ink-2">{check.detail}</p>
            {check.hint && <p className="mt-0.5 text-micro text-ink-3">{check.hint}</p>}
          </dd>
        </div>
      ))}
    </dl>
  );
}

function SelfTestStep({
  result,
  busy,
  onRun,
}: {
  result: StreamSelfTestReport | null;
  busy: boolean;
  onRun: () => void;
}) {
  return (
    <div className="mt-5">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h5 className="flex items-center gap-2 text-label font-semibold text-ink">
          <Sparkles className="size-3.5 text-ink-3" aria-hidden="true" />
          {t("Self-test")}
        </h5>
        <Button size="sm" variant="default" loading={busy} onClick={onRun}>
          {t("Run self-test")}
        </Button>
      </div>
      {result && (
        <div className="mt-3">
          <p className={`text-label ${result.selfTest.ready ? "text-ok" : "text-ink-2"}`}>
            {result.selfTest.message}
          </p>
          <ul className="mt-2 flex flex-col gap-1.5">
            {result.selfTest.findings.map((finding) => {
              const copy = selfTestCopy(finding.id);
              return (
                <li key={finding.id} className="border-l-2 border-line-2 pl-2">
                  <div className="flex items-center gap-2">
                    <Badge tone={statusTone(finding.status)}>{statusLabel(finding.status)}</Badge>
                    <span className="text-label font-medium text-ink">
                      {copy?.title ?? finding.label}
                    </span>
                  </div>
                  {copy?.fix && <p className="mt-0.5 text-meta leading-relaxed text-ink-3">{copy.fix}</p>}
                </li>
              );
            })}
          </ul>
        </div>
      )}
    </div>
  );
}

function ConfigStep({
  report,
  output,
  interfaceName,
  busy,
  onOutput,
  onInterface,
  onPreview,
  onRevert,
}: {
  report: StreamHostReport;
  output: string | null;
  interfaceName: string | null;
  busy: string | null;
  onOutput: (value: string) => void;
  onInterface: (value: string) => void;
  onPreview: () => void;
  onRevert: () => void;
}) {
  const connected = report.outputs.filter((item) => item.connected);
  const selected = output ?? report.configuredOutput ?? connected[0]?.name ?? null;
  const selectedInterface = interfaceName ?? report.mtu?.interface ?? null;

  return (
    <div className="mt-5 border-t border-line pt-4">
      <h5 className="flex items-center gap-2 text-label font-semibold text-ink">
        <MonitorPlay className="size-3.5 text-ink-3" aria-hidden="true" />
        {t("Sunshine configuration")}
      </h5>
      <p className="mt-1 max-w-[80ch] text-meta leading-relaxed text-ink-3">
        {t("Ludex writes output_name, stream_audio and packet_size. The packet size follows the interface MTU so a WireGuard or NetBird tunnel does not fragment the stream.")}
      </p>

      <div className="mt-3 grid gap-4 lg:grid-cols-2">
        <div>
          <p className="text-meta font-medium text-ink">{t("Captured output")}</p>
          {connected.length === 0 ? (
            <p className="mt-1 text-meta text-danger">{t("No connected output was found.")}</p>
          ) : (
            <div className="mt-1.5 flex flex-wrap gap-1.5">
              {connected.map((item) => (
                <button
                  key={item.name}
                  type="button"
                  onClick={() => onOutput(item.name)}
                  aria-pressed={selected === item.name}
                  className={`border px-2 py-1 font-mono text-meta ${
                    selected === item.name
                      ? "border-signal bg-signal text-on-signal"
                      : "border-line-2 text-ink-2 hover:border-ink-3"
                  }`}
                >
                  {item.name}
                  {item.width && item.height ? ` · ${item.width}×${item.height}` : ""}
                </button>
              ))}
            </div>
          )}
        </div>
        <div>
          <p className="text-meta font-medium text-ink">{t("MTU source")}</p>
          {report.interfaces.filter((item) => item.up).length === 0 ? (
            <p className="mt-1 text-meta text-ink-3">{t("No network interface was detected.")}</p>
          ) : (
            <div className="mt-1.5 flex flex-wrap gap-1.5">
              {report.interfaces
                .filter((item) => item.up)
                .map((item) => (
                  <button
                    key={item.interface}
                    type="button"
                    onClick={() => onInterface(item.interface)}
                    aria-pressed={selectedInterface === item.interface}
                    className={`border px-2 py-1 font-mono text-meta ${
                      selectedInterface === item.interface
                        ? "border-signal bg-signal text-on-signal"
                        : "border-line-2 text-ink-2 hover:border-ink-3"
                    }`}
                  >
                    {item.interface} · MTU {item.mtu} → {packetSizeForMtu(item.mtu)} B
                    {item.tunnel ? ` · ${t("tunnel")}` : ""}
                  </button>
                ))}
            </div>
          )}
          {report.mtu && (
            <p className="mt-2 text-meta text-ink-3">
              {t("Packet size:")}{" "}
              <span className="font-mono text-ink-2">
                {packetSizeForMtu(
                  report.interfaces.find((item) => item.interface === selectedInterface)?.mtu ??
                    report.mtu.mtu,
                )}
              </span>{" "}
              {t("bytes")}
            </p>
          )}
        </div>
      </div>

      <div className="mt-4 flex flex-wrap gap-2">
        <Button
          size="sm"
          variant="default"
          loading={busy === "plan"}
          disabled={!report.detected}
          onClick={onPreview}
        >
          {t("Preview sunshine.conf changes")}
        </Button>
        {report.configRevertible && (
          <Button
            size="sm"
            variant="ghost"
            icon={<RotateCcw className="size-3.5" />}
            loading={busy === "revert-config"}
            onClick={onRevert}
          >
            {t("Revert sunshine.conf")}
          </Button>
        )}
      </div>
    </div>
  );
}

function ConfigPlanBody({ plan }: { plan: StreamConfigPlan }) {
  return (
    <div className="flex flex-col gap-3 text-label text-ink-2">
      <div>
        <p className="font-medium text-ink">{t("Changes")}</p>
        <ul className="mt-1 flex flex-col gap-1 font-mono text-meta">
          {plan.edits.map((edit) => (
            <li key={edit.key} className="break-all">
              {editSummary(edit)}
            </li>
          ))}
        </ul>
      </div>
      <div>
        <p className="font-medium text-ink">{t("Files")}</p>
        <ul className="mt-1 flex flex-col gap-1 text-meta">
          {plan.files.map((file) => (
            <li key={`${file.action}-${file.path}`} className="break-all">
              <span className="font-mono text-ink-3">{file.action}</span> {file.path}{" "}
              <span className="text-ink-3">({file.bytes} B)</span>
            </li>
          ))}
        </ul>
      </div>
      <div>
        <p className="font-medium text-ink">{t("Commands")}</p>
        <pre className="mt-1 overflow-x-auto border border-line bg-surface-2 p-2 font-mono text-micro leading-relaxed">
          {plan.commands.join("\n")}
        </pre>
      </div>
      {plan.warnings.map((warning) => (
        <p key={warning} className="border-l-2 border-warn pl-2 text-meta text-ink-3">
          {warning}
        </p>
      ))}
      {plan.blocked && <p className="border-l-2 border-danger pl-2 text-meta text-danger">{plan.blocked}</p>}
      {!plan.applicable && !plan.blocked && <p className="text-meta text-ink-3">{plan.message}</p>}
    </div>
  );
}

function FirewallStep({
  plan,
  busy,
  onOpen,
  onRevert,
}: {
  plan: FirewallPlan | undefined;
  busy: string | null;
  onOpen: () => void;
  onRevert: () => void;
}) {
  return (
    <div className="mt-5 border-t border-line pt-4">
      <h5 className="flex items-center gap-2 text-label font-semibold text-ink">
        <ShieldCheck className="size-3.5 text-ink-3" aria-hidden="true" />
        {t("Firewall")}
      </h5>
      {plan ? (
        <>
          <p className="mt-1 text-meta leading-relaxed text-ink-3">
            {t("Detected:")} <span className="font-mono text-ink-2">{plan.label}</span>
            {" · "}
            <span className="font-mono text-ink-2">{portSummary(plan.rules)}</span>
          </p>
          {plan.blocked && <p className="mt-2 text-meta text-danger">{plan.blocked}</p>}
          <div className="mt-3 flex flex-wrap gap-2">
            <Button
              size="sm"
              variant="default"
              loading={busy === "firewall"}
              disabled={!plan.supported || Boolean(plan.blocked)}
              onClick={onOpen}
            >
              {t("Open Sunshine ports")}
            </Button>
            {(plan.applied || busy === "revert-firewall") && (
              <Button
                size="sm"
                variant="ghost"
                icon={<RotateCcw className="size-3.5" />}
                loading={busy === "revert-firewall"}
                onClick={onRevert}
              >
                {t("Close Ludex rules")}
              </Button>
            )}
          </div>
        </>
      ) : (
        <Skeleton className="mt-2 h-4 w-1/2" />
      )}
    </div>
  );
}

function FirewallBody({ plan }: { plan: FirewallPlan }) {
  return (
    <div className="flex flex-col gap-3 text-label text-ink-2">
      <p className="font-mono text-meta text-ink">{portSummary(plan.rules)}</p>
      <ul className="flex flex-col gap-1 text-meta">
        {plan.rules.map((rule) => (
          <li key={`${rule.protocol}-${rule.first}-${rule.last}`}>
            <span className="font-mono text-ink">{rule.protocol.toUpperCase()}</span>{" "}
            {rule.first === rule.last ? rule.first : `${rule.first}-${rule.last}`} — {rule.purpose}
          </li>
        ))}
      </ul>
      <div>
        <p className="font-medium text-ink">{t("Commands")}</p>
        <pre className="mt-1 overflow-x-auto border border-line bg-surface-2 p-2 font-mono text-micro leading-relaxed">
          {plan.commands.join("\n")}
        </pre>
      </div>
      {plan.blocked && <p className="border-l-2 border-danger pl-2 text-meta text-danger">{plan.blocked}</p>}
    </div>
  );
}

function PairingStep({
  stored,
  pin,
  username,
  password,
  remember,
  busy,
  outcome,
  onPin,
  onUsername,
  onPassword,
  onRemember,
  onPair,
  onOpenPage,
}: {
  stored: boolean;
  pin: string;
  username: string;
  password: string;
  remember: boolean;
  busy: boolean;
  outcome: StreamPairingOutcome | null;
  onPin: (value: string) => void;
  onUsername: (value: string) => void;
  onPassword: (value: string) => void;
  onRemember: (value: boolean) => void;
  onPair: () => void;
  onOpenPage: () => void;
}) {
  return (
    <div className="mt-5 border-t border-line pt-4">
      <h5 className="flex items-center gap-2 text-label font-semibold text-ink">
        <KeyRound className="size-3.5 text-ink-3" aria-hidden="true" />
        {t("Pair Moonlight")}
      </h5>
      <p className="mt-1 max-w-[80ch] text-meta leading-relaxed text-ink-3">
        {t("Start the stream in Moonlight, then type the 4-digit PIN it shows here. Ludex sends it to Sunshine's local API; your credentials stay on this machine.")}
      </p>
      <div className="mt-3 flex flex-wrap items-end gap-2">
        <label className="flex w-[110px] flex-col gap-1">
          <span className="text-meta text-ink-3">{t("PIN")}</span>
          <Input
            value={pin}
            onChange={(event) => onPin(event.target.value.replace(/\D/g, "").slice(0, 4))}
            inputMode="numeric"
            autoComplete="one-time-code"
            placeholder="0000"
            className="h-9 font-mono"
          />
        </label>
        <label className="flex min-w-[150px] flex-1 flex-col gap-1">
          <span className="text-meta text-ink-3">{t("Sunshine username")}</span>
          <Input
            value={username}
            onChange={(event) => onUsername(event.target.value)}
            autoComplete="username"
            placeholder={stored ? t("saved") : "sunshine"}
            className="h-9"
          />
        </label>
        <label className="flex min-w-[150px] flex-1 flex-col gap-1">
          <span className="text-meta text-ink-3">{t("Sunshine password")}</span>
          <Input
            value={password}
            onChange={(event) => onPassword(event.target.value)}
            type="password"
            autoComplete="current-password"
            placeholder={stored ? t("saved") : ""}
            className="h-9"
          />
        </label>
        <label className="flex items-center gap-2 pb-2 text-meta text-ink-3">
          <input
            type="checkbox"
            checked={remember}
            onChange={(event) => onRemember(event.target.checked)}
          />
          {t("Remember")}
        </label>
      </div>
      <div className="mt-3 flex flex-wrap gap-2">
        <Button
          size="sm"
          variant="primary"
          loading={busy}
          disabled={pin.length !== 4}
          onClick={onPair}
        >
          {t("Pair")}
        </Button>
        <Button
          size="sm"
          variant="default"
          icon={<ExternalLink className="size-3.5" />}
          loading={busy}
          onClick={onOpenPage}
        >
          {t("Open the Sunshine page")}
        </Button>
      </div>
      {outcome && (
        <div
          className={`mt-3 border-l-2 pl-3 text-meta leading-relaxed ${
            outcome.mode === "paired" ? "border-ok text-ink-2" : "border-warn text-ink-2"
          }`}
        >
          <p className="flex items-center gap-2">
            {outcome.mode === "paired" ? (
              <CheckCircle2 className="size-3.5 text-ok" aria-hidden="true" />
            ) : (
              <Lock className="size-3.5 text-warn" aria-hidden="true" />
            )}
            {outcome.message}
          </p>
          {outcome.mode !== "paired" && (
            <button
              type="button"
              className="mt-1 inline-flex items-center gap-1 text-meta text-signal underline"
              onClick={onOpenPage}
            >
              {t("Open the Sunshine page for this PIN")}
              <ExternalLink className="size-3" aria-hidden="true" />
            </button>
          )}
        </div>
      )}
    </div>
  );
}
