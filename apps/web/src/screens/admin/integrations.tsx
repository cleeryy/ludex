import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { AlertTriangle, ExternalLink, KeyRound, RefreshCw, ShieldCheck, Trash2 } from "lucide-react";
import { api, USING_MOCKS } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { formatDate, formatRelative } from "@/lib/format";
import type { VirusTotalIntegration } from "@/lib/types";
import {
  VIRUSTOTAL_DAILY_LIMIT,
  VIRUSTOTAL_RATE_PER_MINUTE,
  classifyIntegrationError,
  integrationErrorLabel,
  quotaUsedPercent,
} from "@/lib/virustotal";
import { useToast } from "@/state/toasts";
import { ErrorState, PageHeader, SectionHeader } from "@/components/page";
import { Badge, Button, Field, Input, Progress, Skeleton } from "@/components/ui/controls";
import { ConfirmDialog } from "@/components/ui/dialog";
import { Select } from "@/components/ui/select";
import { t } from "@/lib/i18n";

const VT_DOCS_URL = "https://docs.virustotal.com/reference/overview";

/**
 * Admin → Integrations → VirusTotal (#224).
 *
 * One stored key, its provenance, the public quota and the last failure. The
 * page never shows the key — only the masked fingerprint the server returns —
 * and an env-provided key is read-only here because the server prefers it.
 */
export function VirusTotalIntegrationCard() {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const [apiKey, setApiKey] = useState("");
  const [deleting, setDeleting] = useState(false);

  const { data, isPending, isError, error, refetch } = useQuery({
    queryKey: ["admin-virustotal"],
    queryFn: () => api.adminVirusTotal(),
  });

  const invalidate = () => {
    void queryClient.invalidateQueries({ queryKey: ["admin-virustotal"] });
  };

  const test = useMutation({
    mutationFn: () => api.testVirusTotalKey(apiKey.trim() || undefined),
    onSuccess: (result) => {
      push({
        title: result.ok ? t("VirusTotal key accepted") : t("VirusTotal rejected this key"),
        description: result.message,
        tone: result.ok ? "success" : "danger",
      });
    },
    onError: (cause: Error) =>
      push({ title: t("Could not test the key"), description: errorMessage(cause), tone: "danger" }),
  });

  const save = useMutation({
    mutationFn: () => api.saveVirusTotalKey({ apiKey: apiKey.trim() }),
    onSuccess: () => {
      setApiKey("");
      invalidate();
      push({ title: t("VirusTotal key saved"), description: t("Hash lookups now run against the public API.") });
    },
    onError: (cause: Error) =>
      push({ title: t("Could not save the key"), description: errorMessage(cause), tone: "danger" }),
  });

  const remove = useMutation({
    mutationFn: () => api.deleteVirusTotalKey(),
    onSuccess: () => {
      setDeleting(false);
      invalidate();
      push({ title: t("VirusTotal key deleted"), description: t("Every verdict now reports as disabled.") });
    },
    onError: (cause: Error) =>
      push({ title: t("Could not delete the key"), description: errorMessage(cause), tone: "danger" }),
  });

  if (isError) return <ErrorState message={errorMessage(error)} onRetry={() => void refetch()} />;
  if (isPending || !data) return <Skeleton className="mt-6 h-64 w-full" />;

  return (
    <VirusTotalCardBody
      data={data}
      apiKey={apiKey}
      onApiKeyChange={setApiKey}
      testPending={test.isPending}
      savePending={save.isPending}
      onTest={() => test.mutate()}
      onSave={() => save.mutate()}
      onDelete={() => setDeleting(true)}
      deleting={deleting}
      onDeleteOpenChange={setDeleting}
      deletePending={remove.isPending}
      onConfirmDelete={() => remove.mutate()}
    />
  );
}

function VirusTotalCardBody({
  data,
  apiKey,
  onApiKeyChange,
  testPending,
  savePending,
  onTest,
  onSave,
  onDelete,
  deleting,
  onDeleteOpenChange,
  deletePending,
  onConfirmDelete,
}: {
  data: VirusTotalIntegration;
  apiKey: string;
  onApiKeyChange: (value: string) => void;
  testPending: boolean;
  savePending: boolean;
  onTest: () => void;
  onSave: () => void;
  onDelete: () => void;
  deleting: boolean;
  onDeleteOpenChange: (open: boolean) => void;
  deletePending: boolean;
  onConfirmDelete: () => void;
}) {
  const fromEnv = data.source === "env";
  const configured = data.configured;
  const errorKind = classifyIntegrationError(data.lastError);
  const quotaPercent = quotaUsedPercent(data.quota);
  const canSubmit = apiKey.trim().length > 0 && !fromEnv;

  return (
    <div className="flex flex-col gap-8">
      <div className="flex flex-wrap items-center gap-2">
        <Badge tone={configured ? "ok" : "outline"}>
          {configured ? t("Configured") : t("Disabled")}
        </Badge>
        <Badge tone={fromEnv ? "signal" : "outline"}>
          {fromEnv ? t("Key from environment") : configured ? t("Key stored on the server") : t("No key")}
        </Badge>
        {USING_MOCKS && <Badge tone="outline">{t("Demo data")}</Badge>}
        {data.fingerprint && (
          <span className="font-mono text-[11px] text-ink-3" title={t("Only the last four characters are ever shown")}>
            {data.fingerprint}
          </span>
        )}
      </div>

      {!configured && (
        <p className="flex items-start gap-2 border-l-2 border-signal pl-3 text-[12.5px] leading-relaxed text-signal-text">
          <AlertTriangle className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
          <span>
            {t(
              "VirusTotal is off: every archive reports disabled, and nothing is looked up. Ludex never shows an unchecked archive as clean.",
            )}
          </span>
        </p>
      )}

      {fromEnv && (
        <p className="border-l-2 border-line-2 pl-3 text-[12px] leading-relaxed text-ink-3">
          {t(
            "This key comes from LUDEX_VIRUSTOTAL_API_KEY. The environment always wins, so the form is read-only here: change the variable and restart the server.",
          )}
        </p>
      )}

      <section>
        <SectionHeader
          title={t("API key")}
          description={t(
            "The key is stored encrypted and never returned by the API. Only the last four characters are shown here.",
          )}
        />
        <form
          className="mt-4 flex max-w-[520px] flex-col gap-3"
          onSubmit={(event) => {
            event.preventDefault();
            if (canSubmit) onSave();
          }}
        >
          <Field
            label={fromEnv ? t("API key (read-only)") : t("API key")}
            hint={t("Find it in VirusTotal → your profile → API key. A free key allows {daily} lookups per day.", {
              daily: VIRUSTOTAL_DAILY_LIMIT,
            })}
            htmlFor="virustotal-api-key"
          >
            <Input
              id="virustotal-api-key"
              type="password"
              autoComplete="off"
              spellCheck={false}
              value={apiKey}
              disabled={fromEnv}
              onChange={(event) => onApiKeyChange(event.target.value)}
              placeholder={configured ? t("Paste a new key to replace the stored one") : t("Paste your VirusTotal API key")}
              className="font-mono text-[13px]"
            />
          </Field>
          <div className="flex flex-wrap items-center gap-2">
            <Button
              type="submit"
              variant="primary"
              loading={savePending}
              disabled={!canSubmit}
              icon={<KeyRound className="size-4" />}
            >
              {configured ? t("Replace key") : t("Save key")}
            </Button>
            <Button
              variant="default"
              loading={testPending}
              disabled={!apiKey.trim() && !configured}
              icon={<RefreshCw className="size-4" />}
              onClick={onTest}
            >
              {t("Test key")}
            </Button>
            {configured && !fromEnv && (
              <Button
                variant="ghost"
                className="text-danger"
                icon={<Trash2 className="size-4" />}
                onClick={onDelete}
              >
                {t("Delete key")}
              </Button>
            )}
          </div>
        </form>
      </section>

      <section>
        <SectionHeader
          title={t("Quota")}
          description={t(
            "The public API allows {daily} lookups per day and {rate} per minute. Ludex caches reports and never retries through the limit.",
            { daily: data.quota.dailyLimit, rate: VIRUSTOTAL_RATE_PER_MINUTE },
          )}
        />
        <div className="mt-4 max-w-[520px]">
          <div className="flex items-baseline justify-between gap-3">
            <span className="text-[12.5px] text-ink-2">
              {t("{used} of {limit} used today", { used: data.quota.usedToday, limit: data.quota.dailyLimit })}
            </span>
            <span className="num text-[12px] text-ink-3">{t("{remaining} left", { remaining: data.quota.remaining })}</span>
          </div>
          <Progress
            className="mt-2"
            value={quotaPercent}
            tone={quotaPercent >= 80 ? "danger" : "signal"}
            label={t("Daily VirusTotal quota")}
          />
          <p className="mt-2 font-mono text-[10.5px] text-ink-3">
            {t("Resets {when}", { when: formatRelative(data.quota.resetAt) })}
            {" · "}
            {formatDate(data.quota.resetAt)}
          </p>
        </div>
      </section>

      <section>
        <SectionHeader title={t("Last error")} />
        {data.lastError ? (
          <div className="mt-3 flex max-w-[620px] flex-col gap-1.5 border-l-2 border-danger pl-3">
            <p className="flex flex-wrap items-center gap-2 text-[12.5px] text-ink">
              <Badge tone="danger">{integrationErrorLabel(errorKind) ?? t("Error")}</Badge>
              <span className="font-mono text-[11px] leading-relaxed text-ink-2">{data.lastError}</span>
            </p>
            <p className="text-[11.5px] leading-relaxed text-ink-3">
              {errorKind === "invalid_key"
                ? t("VirusTotal rejected the key. Replace it above; scans keep reporting unknown until then.")
                : errorKind === "quota"
                  ? t("The quota is exhausted. Scans resume after the reset shown above without any manual repair.")
                  : errorKind === "outage"
                    ? t("VirusTotal was unreachable. The verdict is unknown, not clean; retry later.")
                    : t("This is the last failure the server recorded for a VirusTotal call.")}
            </p>
          </div>
        ) : (
          <p className="mt-3 flex items-center gap-2 text-[12.5px] text-ink-3">
            <ShieldCheck className="size-4 text-ok" aria-hidden="true" />
            {data.lastUsedAt
              ? t("No error recorded on the last call.")
              : t("No call has been made yet.")}
          </p>
        )}
        <dl className="mt-4 flex max-w-[520px] flex-col gap-1.5 border-t border-line pt-3 text-[12px]">
          <div className="flex items-baseline justify-between gap-3">
            <dt className="text-ink-3">{t("Key added")}</dt>
            <dd className="text-ink-2">{data.addedAt ? formatRelative(data.addedAt) : "—"}</dd>
          </div>
          <div className="flex items-baseline justify-between gap-3">
            <dt className="text-ink-3">{t("Last used")}</dt>
            <dd className="text-ink-2">{data.lastUsedAt ? formatRelative(data.lastUsedAt) : t("Never")}</dd>
          </div>
        </dl>
      </section>

      <section>
        <SectionHeader title={t("What scanning does")} />
        <ul className="mt-3 flex max-w-[620px] flex-col gap-2 text-[12.5px] leading-relaxed text-ink-2">
          <li className="flex gap-2.5">
            <span className="mt-[7px] size-1.5 shrink-0 bg-signal" aria-hidden="true" />
            <span>{t("A scan looks the archive SHA-256 up. It never uploads, extracts or runs a file.")}</span>
          </li>
          <li className="flex gap-2.5">
            <span className="mt-[7px] size-1.5 shrink-0 bg-signal" aria-hidden="true" />
            <span>{t("Uploading is a separate, per-file action: 32 MB direct, 650 MB through an upload URL, bigger archives refused.")}</span>
          </li>
          <li className="flex gap-2.5">
            <span className="mt-[7px] size-1.5 shrink-0 bg-signal" aria-hidden="true" />
            <span>{t("Uploaded files are shared publicly. Nothing is ever uploaded automatically.")}</span>
          </li>
          <li className="flex gap-2.5">
            <span className="mt-[7px] size-1.5 shrink-0 bg-signal" aria-hidden="true" />
            <span>{t("A verdict is advisory: Ludex never deletes, quarantines or blocks a game from it.")}</span>
          </li>
        </ul>
        <a
          href={VT_DOCS_URL}
          target="_blank"
          rel="noreferrer"
          className="mt-3 inline-flex items-center gap-1.5 text-[12px] text-ink-3 hover:text-ink"
        >
          {t("VirusTotal API documentation")}
          <ExternalLink className="size-3" aria-hidden="true" />
        </a>
      </section>

      <ConfirmDialog
        open={deleting}
        onOpenChange={onDeleteOpenChange}
        title={t("Delete the VirusTotal key?")}
        description={t(
          "Every archive reports disabled until a new key is saved. Existing reports stay cached but stop refreshing.",
        )}
        confirmLabel={t("Delete key")}
        loading={deletePending}
        onConfirm={onConfirmDelete}
      />
    </div>
  );
}

/** Demo switch for the mock build: every badge state is one click away. */
export function VirusTotalDemoSwitch() {
  const [scenario, setScenario] = useState("mixed");
  useEffect(() => {
    try {
      setScenario(localStorage.getItem("ludex.demo.virustotal") ?? "mixed");
    } catch {
      // Private mode: keep the in-memory default.
    }
  }, []);

  return (
    <div className="mt-4 max-w-[320px]">
      <Field label={t("Demo verdict state")} hint={t("Changes what the mock API reports for every archive.")}>
        <Select
          label={t("Demo verdict state")}
          value={scenario}
          onChange={(value) => {
            setScenario(value);
            try {
              if (value === "mixed") localStorage.removeItem("ludex.demo.virustotal");
              else localStorage.setItem("ludex.demo.virustotal", value);
            } catch {
              // Ignore storage failures in private mode.
            }
          }}
          options={[
            { value: "mixed", label: t("Mixed (default)") },
            { value: "clean", label: t("Clean") },
            { value: "suspicious", label: t("Suspicious") },
            { value: "malicious", label: t("Malicious") },
            { value: "not_found", label: t("Never seen") },
            { value: "unknown", label: t("Not checked") },
            { value: "disabled", label: t("Disabled") },
            { value: "quota_exceeded", label: t("Quota reached") },
            { value: "error", label: t("Check failed") },
          ]}
          className="h-9 text-[12.5px]"
        />
      </Field>
    </div>
  );
}

/** The Integrations page body; only VirusTotal exists today. */
export function IntegrationsScreen() {
  return (
    <div className="page page-in">
      <PageHeader
        title={t("Integrations")}
        meta={<Badge tone="outline">{t("Server-side keys")}</Badge>}
        description={t(
          "Third-party services the server calls on your behalf. Keys live on the server, never in a browser session, and are masked everywhere.",
        )}
      />
      <section className="mt-8">
        <SectionHeader
          level={2}
          title={t("VirusTotal")}
          description={t("Hash lookups for game archives, with an opt-in per-file upload.")}
        />
        <div className="mt-5">
          <VirusTotalIntegrationCard />
        </div>
        {USING_MOCKS && <VirusTotalDemoSwitch />}
      </section>
    </div>
  );
}
