import { useCallback, useEffect, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useSearchParams } from "react-router-dom";
import { RefreshCw, ShieldCheck } from "lucide-react";
import { errorMessage } from "@/lib/http";
import { hostAvailable, hostBridge, hostGaps, hostInstallableIds } from "@/lib/host";
import { formatDateValue, t, tn } from "@/lib/i18n";
import type { HostCheck, HostInstallPlan, HostInstallReport, HostStatus } from "@/lib/types";
import { Badge, Button } from "@/components/ui/controls";
import { Dialog } from "@/components/ui/dialog";
import { SectionHeader } from "@/components/page";
import { qk } from "@/lib/query-keys";

/**
 * Settings → Host (#285).
 *
 * Every probe is a sentence, grouped by status; nothing is installed without
 * the consent dialog listing the exact packages for the detected distribution.
 * "Re-run checks" is the only thing that re-probes; a launch reads the stored
 * report instead.
 */
export function HostPreflightPanel() {
  const queryClient = useQueryClient();
  const enabled = hostAvailable();
  const report = useQuery({
    queryKey: qk.hostPreflight,
    queryFn: () => hostBridge.report(),
    enabled,
    staleTime: 5 * 60_000,
    retry: false,
  });

  const [consentOpen, setConsentOpen] = useState(false);
  const [pendingIds, setPendingIds] = useState<string[]>([]);
  const [plan, setPlan] = useState<HostInstallPlan | null>(null);
  const [outcome, setOutcome] = useState<HostInstallReport | null>(null);
  const [dialogError, setDialogError] = useState<string | null>(null);
  const [params] = useSearchParams();
  const autoOpened = useRef(false);

  const rerun = useMutation({
    mutationFn: () => hostBridge.report(true),
    onSuccess: (data) => queryClient.setQueryData(qk.hostPreflight, data),
  });

  const install = useMutation({
    mutationFn: () => hostBridge.install(pendingIds, false, true),
    onSuccess: async (result) => {
      setOutcome(result.report);
      if (result.report?.success) {
        const fresh = await hostBridge.report(true);
        queryClient.setQueryData(qk.hostPreflight, fresh);
      }
    },
    onError: (error) => setDialogError(errorMessage(error)),
  });

  const openConsent = useCallback(async () => {
    const ids = hostInstallableIds(report.data);
    if (!ids.length) return;
    setPendingIds(ids);
    setPlan(null);
    setOutcome(null);
    setDialogError(null);
    setConsentOpen(true);
    try {
      setPlan(await hostBridge.plan(ids));
    } catch (error) {
      setDialogError(errorMessage(error));
    }
  }, [report.data]);

  // The first-run prompt links here with `install=1`: one click to the consent
  // list, one to install, so a fresh machine needs no terminal and no hunt.
  useEffect(() => {
    if (autoOpened.current || params.get("install") !== "1") return;
    if (!report.data || hostInstallableIds(report.data).length === 0) return;
    autoOpened.current = true;
    void openConsent();
  }, [openConsent, params, report.data]);

  const closeConsent = () => {
    if (install.isPending) return;
    setConsentOpen(false);
  };

  const data = report.data ?? null;
  const gaps = hostGaps(data);
  const installable = hostInstallableIds(data);
  const groups = groupFindings(data?.findings ?? []);

  return (
    <section className="mt-10" aria-labelledby="host-preflight-heading">
      <SectionHeader
        id="host-preflight-heading"
        title={t("Host readiness")}
        description={t("Graphics, Vulkan and the runtimes games need on this machine")}
        actions={
          <div className="flex items-center gap-2">
            <Button
              variant="ghost"
              size="sm"
              icon={<RefreshCw className="size-3.5" />}
              loading={rerun.isPending || report.isFetching}
              disabled={!enabled}
              onClick={() => rerun.mutate()}
            >
              {t("Re-run checks")}
            </Button>
            <Button
              size="sm"
              icon={<ShieldCheck className="size-3.5" />}
              disabled={!enabled || installable.length === 0}
              onClick={() => void openConsent()}
            >
              {t("Install missing")}
            </Button>
          </div>
        }
      />
      <div className="border-t border-line pt-4">
        {!enabled && (
          <p className="text-label text-ink-3">
            {t("Host checks need the Ludex desktop app on Linux.")}
          </p>
        )}

        {report.isError && (
          <p role="alert" className="mb-3 text-label text-danger">
            {t("The host checks failed. The desktop app reports the reason in its log.")}
          </p>
        )}

        {data && (
          <>
            <div className="flex flex-wrap items-baseline gap-x-5 gap-y-1 font-mono text-micro text-ink-3">
              <span>{data.family.label}</span>
              <span>{data.family.detail}</span>
              <span>
                {t("Checked:")} {formatDateValue(data.checkedAt, { dateStyle: "medium", timeStyle: "short" })}
              </span>
              <span>
                {data.ready
                  ? t("Ready to play")
                  : tn(gaps.length, "1 finding needs attention", "{count} findings need attention")}
              </span>
            </div>

            {data.warnings.map((warning) => (
              <p key={warning} className="mt-2 border-l-2 border-signal pl-2.5 text-label text-ink-2">
                {warning}
              </p>
            ))}

            {groups.map((group) => (
              <div key={group.status} className="mt-6">
                <h3 className="mb-2 flex items-baseline gap-2 text-label font-medium text-ink-2">
                  {statusLabel(group.status)}
                  <span className="num text-meta text-ink-3">{group.findings.length}</span>
                </h3>
                <ul className="border-t border-line">
                  {group.findings.map((finding) => (
                    <FindingRow key={finding.id} finding={finding} />
                  ))}
                </ul>
              </div>
            ))}

            <p className="mt-4 max-w-[80ch] text-meta leading-relaxed text-ink-3">
              {t(
                "Ludex never installs GPU drivers: it detects them, explains what is missing and links to your distribution's instructions.",
              )}
            </p>
          </>
        )}
      </div>

      <Dialog
        open={consentOpen}
        onOpenChange={(open) => {
          if (!open) closeConsent();
        }}
        title={t("Install host packages")}
        description={t("This runs exactly the command below, with your permission.")}
        footer={
          <div className="flex flex-wrap items-center justify-end gap-2">
            <Button variant="ghost" size="sm" disabled={install.isPending} onClick={closeConsent}>
              {outcome?.success ? t("Close") : t("Cancel")}
            </Button>
            <Button
              size="sm"
              loading={install.isPending}
              disabled={!plan || plan.command.length === 0 || outcome?.success === true}
              onClick={() => install.mutate()}
            >
              {t("Install packages")}
            </Button>
          </div>
        }
      >
        <div className="space-y-4 text-label leading-relaxed text-ink-2">
          {dialogError && (
            <p role="alert" className="border-l-2 border-danger pl-2.5 text-danger">
              {dialogError}
            </p>
          )}
          {!plan && !dialogError && <p className="text-ink-3">{t("Building the command…")}</p>}
          {plan && (
            <>
              <p>
                {t("Packages for")} <span className="font-medium text-ink">{plan.family.label}</span>
              </p>
              {plan.packages.length > 0 ? (
                <ul className="border-t border-line">
                  {plan.packages.map((entry) => (
                    <li
                      key={`${entry.finding}-${entry.package}`}
                      className="flex items-baseline justify-between gap-4 border-b border-line py-1.5"
                    >
                      <span className="text-ink-2">{entry.label}</span>
                      <span className="font-mono text-meta text-ink">{entry.package}</span>
                    </li>
                  ))}
                </ul>
              ) : (
                <p className="text-ink-3">{t("No package can be installed for this distribution.")}</p>
              )}
              {plan.command.length > 0 && (
                <div>
                  <p className="mb-1 text-micro uppercase tracking-[0.08em] text-ink-3">
                    {t("Command")}
                  </p>
                  <pre className="overflow-x-auto border border-line bg-surface p-2.5 font-mono text-meta text-ink">
                    {plan.commandLabel}
                  </pre>
                </div>
              )}
              {plan.manual.length > 0 && (
                <div>
                  <p className="mb-1 text-micro uppercase tracking-[0.08em] text-ink-3">
                    {t("Manual steps")}
                  </p>
                  <ul className="space-y-1.5">
                    {plan.manual.map((step) => (
                      <li key={step.finding}>
                        <span className="text-ink">{step.label}: </span>
                        {step.instruction}{" "}
                        {step.url && (
                          <a
                            href={step.url}
                            target="_blank"
                            rel="noreferrer"
                            className="text-signal-text underline decoration-line-2 underline-offset-2"
                          >
                            {t("Official instructions")}
                          </a>
                        )}
                      </li>
                    ))}
                  </ul>
                </div>
              )}
              {plan.refused.length > 0 && (
                <ul className="space-y-1">
                  {plan.refused.map((refusal) => (
                    <li key={refusal.finding} className="text-ink-3">
                      {refusal.reason}
                    </li>
                  ))}
                </ul>
              )}
              {plan.notes.map((note) => (
                <p key={note} className="text-ink-3">
                  {note}
                </p>
              ))}
              <p className="text-meta text-ink-3">
                {t("Nothing is installed until you press Install packages.")}
              </p>
              {outcome && (
                <div>
                  <p
                    role="status"
                    className={outcome.success ? "text-ok" : "text-danger"}
                  >
                    {outcome.success
                      ? t("Packages installed. The checks ran again.")
                      : outcome.cancelled
                        ? t("The permission prompt was dismissed.")
                        : t("The installation failed.")}
                  </p>
                  {outcome.output && (
                    <pre className="mt-2 max-h-48 overflow-auto border border-line bg-surface p-2.5 font-mono text-micro text-ink-3">
                      {outcome.output}
                    </pre>
                  )}
                </div>
              )}
            </>
          )}
        </div>
      </Dialog>
    </section>
  );
}

const GROUP_ORDER: HostStatus[] = ["missing", "tooOld", "ok", "unknown"];

function groupFindings(findings: HostCheck[]) {
  return GROUP_ORDER.map((status) => ({
    status,
    findings: findings.filter((finding) => finding.status === status),
  })).filter((group) => group.findings.length > 0);
}

function statusLabel(status: HostStatus): string {
  switch (status) {
    case "missing":
      return t("Missing");
    case "tooOld":
      return t("Too old");
    case "ok":
      return t("Ready");
    case "unknown":
      return t("Unknown");
  }
}

function statusTone(status: HostStatus): "danger" | "signal" | "ok" | "outline" {
  switch (status) {
    case "missing":
      return "danger";
    case "tooOld":
      return "signal";
    case "ok":
      return "ok";
    case "unknown":
      return "outline";
  }
}

function FindingRow({ finding }: { finding: HostCheck }) {
  return (
    <li className="border-b border-line py-2.5">
      <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
        <Badge tone={statusTone(finding.status)}>{statusLabel(finding.status)}</Badge>
        <span className="text-label font-medium text-ink">{finding.label}</span>
        {finding.installable && finding.packages.length > 0 && (
          <span className="font-mono text-micro text-ink-3">{finding.packages.join(" ")}</span>
        )}
      </div>
      <p className="mt-1 max-w-[80ch] text-label leading-relaxed text-ink-2">{finding.detail}</p>
      {finding.hint && (
        <p className="mt-0.5 max-w-[80ch] truncate font-mono text-micro text-ink-3" title={finding.hint}>
          {finding.hint}
        </p>
      )}
      {finding.manual && (
        <p className="mt-1 max-w-[80ch] text-meta leading-relaxed text-ink-3">
          {finding.manual.instruction}
          {finding.manual.url && (
            <>
              {" "}
              <a
                href={finding.manual.url}
                target="_blank"
                rel="noreferrer"
                className="text-signal-text underline decoration-line-2 underline-offset-2"
              >
                {t("Official instructions")}
              </a>
            </>
          )}
        </p>
      )}
    </li>
  );
}
