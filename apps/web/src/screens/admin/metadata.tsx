import { useEffect, useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  ArrowDown,
  ArrowUp,
  ExternalLink,
  KeyRound,
  Pencil,
  Plus,
  PlugZap,
  Trash2,
} from "lucide-react";
import { api, USING_MOCKS } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import type {
  MetadataProvider,
  MetadataProviderField,
  MetadataProviderInput,
  ProviderCandidate,
  ProviderTestResult,
} from "@/lib/types";
import { useToast } from "@/state/toasts";
import { ErrorState, PageHeader, SectionHeader } from "@/components/page";
import { Badge, Button, Field, Input, Skeleton, Switch } from "@/components/ui/controls";
import { ConfirmDialog, Dialog } from "@/components/ui/dialog";
import { t } from "@/lib/i18n";

const DOCS_URL = "https://github.com/cleeryy/ludex/blob/main/docs/INTEGRATIONS.md";

const CANONICAL_FIELDS: MetadataProviderField[] = [
  "id",
  "title",
  "description",
  "releaseDate",
  "coverUrl",
  "backgroundUrl",
  "screenshots",
  "genres",
  "developers",
  "publishers",
  "tags",
  "rating",
  "ageRating",
  "providerUrl",
];

interface FormState {
  name: string;
  baseUrl: string;
  token: string;
  clearToken: boolean;
  priority: string;
  enabled: boolean;
  mapping: string;
}

function emptyForm(priority: number): FormState {
  return {
    name: "",
    baseUrl: "",
    token: "",
    clearToken: false,
    priority: String(priority),
    enabled: false,
    mapping: "{}",
  };
}

function formFrom(provider: MetadataProvider): FormState {
  return {
    name: provider.name,
    baseUrl: provider.baseUrl,
    token: "",
    clearToken: false,
    priority: String(provider.priority),
    enabled: provider.enabled,
    mapping: JSON.stringify(provider.mapping ?? {}, null, 2),
  };
}

/** Parses and validates the mapping editor. Returns the mapping or an error. */
function parseMapping(raw: string): { mapping: Partial<Record<MetadataProviderField, string>> } | { error: string } {
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    return { error: t("The mapping is not valid JSON.") };
  }
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    return { error: t("The mapping must be a JSON object.") };
  }
  const mapping: Partial<Record<MetadataProviderField, string>> = {};
  for (const [key, path] of Object.entries(value as Record<string, unknown>)) {
    if (!CANONICAL_FIELDS.includes(key as MetadataProviderField)) {
      return { error: t("Unknown field in the mapping: {field}", { field: key }) };
    }
    if (typeof path !== "string" || !path.trim()) {
      return { error: t("The mapping path for {field} must be a non-empty string.", { field: key }) };
    }
    if (!/^[A-Za-z0-9_-]+(\.[A-Za-z0-9_-]+)*$/.test(path.trim())) {
      return {
        error: t("The mapping path for {field} must be dot-separated object keys.", { field: key }),
      };
    }
    mapping[key as MetadataProviderField] = path.trim();
  }
  return { mapping };
}

function defaultPriority(providers: MetadataProvider[]): number {
  const highest = providers.reduce((max, provider) => Math.max(max, provider.priority), 0);
  return Math.min(1_000_000, highest + 10);
}

export function MetadataProvidersScreen() {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const [editing, setEditing] = useState<MetadataProvider | null>(null);
  const [creatingOpen, setCreatingOpen] = useState(false);
  const [deleting, setDeleting] = useState<MetadataProvider | null>(null);
  const [testing, setTesting] = useState<MetadataProvider | null>(null);

  const { data, isPending, isError, error, refetch } = useQuery({
    queryKey: ["admin-metadata-providers"],
    queryFn: () => api.adminMetadataProviders(),
  });

  const providers = data ?? [];

  const invalidate = () => {
    void queryClient.invalidateQueries({ queryKey: ["admin-metadata-providers"] });
  };

  const reorder = useMutation({
    mutationFn: (ids: number[]) => api.reorderMetadataProviders(ids),
    onSuccess: (updated) => {
      queryClient.setQueryData(["admin-metadata-providers"], updated);
      push({ title: t("Provider order saved") });
    },
    onError: (cause: Error) =>
      push({ title: t("Could not reorder the providers"), description: cause.message, tone: "danger" }),
  });

  const remove = useMutation({
    mutationFn: (provider: MetadataProvider) => api.deleteMetadataProvider(provider.id),
    onSuccess: (_, provider) => {
      setDeleting(null);
      invalidate();
      push({ title: t("Provider deleted"), description: provider.name });
    },
    onError: (cause: Error) =>
      push({ title: t("Could not delete the provider"), description: cause.message, tone: "danger" }),
  });

  const move = (provider: MetadataProvider, direction: -1 | 1) => {
    const index = providers.findIndex((entry) => entry.id === provider.id);
    const target = index + direction;
    if (index < 0 || target < 0 || target >= providers.length) return;
    const ids = providers.map((entry) => entry.id);
    [ids[index], ids[target]] = [ids[target], ids[index]];
    reorder.mutate(ids);
  };

  return (
    <div className="page page-in">
      <PageHeader
        title={t("Metadata providers")}
        meta={
          <>
            <Badge tone="outline">{t("IGDB is always tried first")}</Badge>
            <Badge tone={providers.some((provider) => provider.enabled) ? "ok" : "outline"}>
              {providers.filter((provider) => provider.enabled).length} {t("enabled")}
            </Badge>
            {USING_MOCKS && <Badge tone="outline">{t("Demo data")}</Badge>}
          </>
        }
        description={t(
          "Community metadata sources run as HTTP sidecars. No third-party code runs inside the server: configure a base URL, an optional token and a field mapping.",
        )}
        actions={
          <>
            <a
              href={DOCS_URL}
              target="_blank"
              rel="noreferrer"
              className="flex h-9 items-center gap-1.5 text-[12.5px] text-ink-2 hover:text-ink"
            >
              {t("Provider contract")}
              <ExternalLink className="size-3.5" aria-hidden="true" />
            </a>
            <Button
              variant="primary"
              icon={<Plus className="size-4" />}
              onClick={() => setCreatingOpen(true)}
            >
              {t("Add provider")}
            </Button>
          </>
        }
      />

      {isError && <ErrorState message={errorMessage(error)} onRetry={() => void refetch()} />}

      {isPending && (
        <div className="mt-8 flex flex-col gap-3">
          {Array.from({ length: 3 }).map((_, index) => (
            <Skeleton key={index} className="h-20 w-full" />
          ))}
        </div>
      )}

      {!isPending && !isError && (
        <section className="mt-8">
          <SectionHeader
            title={t("Provider chain")}
            description={t(
              "Providers run after IGDB and before the Steam and file-name fallbacks, in ascending priority order. Provider fields only fill metadata that is still empty.",
            )}
          />

          {providers.length === 0 ? (
            <p className="mt-5 border border-line bg-surface/50 p-5 text-[12.5px] text-ink-2">
              {t("No community provider is configured. IGDB, Steam and file names keep working as before.")}
            </p>
          ) : (
            <ul className="mt-5 divide-y divide-line border border-line">
              {providers.map((provider, index) => (
                <li key={provider.id} className="flex flex-wrap items-center gap-4 p-4">
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-2">
                      <p className="truncate text-[13.5px] font-medium text-ink">{provider.name}</p>
                      <Badge tone={provider.enabled ? "ok" : "outline"}>
                        {provider.enabled ? t("Enabled") : t("Disabled")}
                      </Badge>
                      {provider.secretConfigured && (
                        <Badge tone="outline">
                          <KeyRound className="mr-1 size-3" aria-hidden="true" />
                          {t("Token stored")}
                        </Badge>
                      )}
                    </div>
                    <p className="mt-1 truncate font-mono text-[11.5px] text-ink-3">{provider.baseUrl}</p>
                    {Object.keys(provider.mapping ?? {}).length > 0 && (
                      <p className="mt-1 text-[11.5px] text-ink-3">
                        {t("{count} mapped fields", { count: Object.keys(provider.mapping ?? {}).length })}
                      </p>
                    )}
                  </div>

                  <div className="flex items-center gap-2">
                    <span className="num w-16 text-right text-[12px] text-ink-3">
                      {t("Priority {priority}", { priority: provider.priority })}
                    </span>
                    <Button
                      variant="ghost"
                      size="sm"
                      aria-label={t("Move {name} up", { name: provider.name })}
                      disabled={index === 0 || reorder.isPending}
                      onClick={() => move(provider, -1)}
                    >
                      <ArrowUp className="size-3.5" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="sm"
                      aria-label={t("Move {name} down", { name: provider.name })}
                      disabled={index === providers.length - 1 || reorder.isPending}
                      onClick={() => move(provider, 1)}
                    >
                      <ArrowDown className="size-3.5" />
                    </Button>
                    <Button
                      variant="default"
                      size="sm"
                      icon={<PlugZap className="size-3.5" />}
                      onClick={() => setTesting(provider)}
                    >
                      {t("Test")}
                    </Button>
                    <Button
                      variant="default"
                      size="sm"
                      icon={<Pencil className="size-3.5" />}
                      onClick={() => setEditing(provider)}
                    >
                      {t("Edit")}
                    </Button>
                    <Button
                      variant="ghost"
                      size="sm"
                      className="text-danger"
                      aria-label={t("Delete {name}", { name: provider.name })}
                      onClick={() => setDeleting(provider)}
                    >
                      <Trash2 className="size-3.5" />
                    </Button>
                  </div>
                </li>
              ))}
            </ul>
          )}
        </section>
      )}

      <ProviderDialog
        open={creatingOpen}
        provider={null}
        defaultPriority={defaultPriority(providers)}
        onOpenChange={setCreatingOpen}
        onSaved={invalidate}
      />
      <ProviderDialog
        open={editing !== null}
        provider={editing}
        defaultPriority={defaultPriority(providers)}
        onOpenChange={(open) => {
          if (!open) setEditing(null);
        }}
        onSaved={invalidate}
      />
      <TestDialog provider={testing} onOpenChange={(open) => !open && setTesting(null)} />

      <ConfirmDialog
        open={deleting !== null}
        onOpenChange={(open) => !open && setDeleting(null)}
        title={t("Delete this provider?")}
        description={t(
          "The provider configuration and its fetch history are removed. Existing game metadata is kept and IGDB, Steam and file names keep working.",
        )}
        confirmLabel={t("Delete provider")}
        tone="danger"
        loading={remove.isPending}
        onConfirm={() => {
          if (deleting) remove.mutate(deleting);
        }}
      />
    </div>
  );
}

function ProviderDialog({
  open,
  provider,
  defaultPriority,
  onOpenChange,
  onSaved,
}: {
  open: boolean;
  provider: MetadataProvider | null;
  defaultPriority: number;
  onOpenChange: (open: boolean) => void;
  onSaved: () => void;
}) {
  const { push } = useToast();
  const [form, setForm] = useState<FormState>(emptyForm(defaultPriority));
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    setForm(provider ? formFrom(provider) : emptyForm(defaultPriority));
    setError(null);
  }, [open, provider, defaultPriority]);

  const mappingResult = useMemo(() => parseMapping(form.mapping), [form.mapping]);
  const priority = Number(form.priority);
  const priorityValid = Number.isInteger(priority) && priority >= -1_000_000 && priority <= 1_000_000;

  const save = useMutation({
    mutationFn: (body: MetadataProviderInput) =>
      provider ? api.updateMetadataProvider(provider.id, body) : api.createMetadataProvider(body),
    onSuccess: () => {
      onOpenChange(false);
      onSaved();
      push({ title: provider ? t("Provider updated") : t("Provider created") });
    },
    onError: (cause: Error) => setError(errorMessage(cause)),
  });

  const set = <K extends keyof FormState>(key: K, value: FormState[K]) =>
    setForm((current) => ({ ...current, [key]: value }));

  const submit = (event: React.FormEvent) => {
    event.preventDefault();
    setError(null);
    if (!form.name.trim()) return setError(t("Provider name is required"));
    if (!/^https?:\/\//i.test(form.baseUrl.trim())) {
      return setError(t("The base URL must be an absolute http(s) URL"));
    }
    if ("error" in mappingResult) return setError(mappingResult.error);
    if (!priorityValid) return setError(t("Priority must be a whole number between -1000000 and 1000000"));

    const body: MetadataProviderInput = {
      name: form.name.trim(),
      baseUrl: form.baseUrl.trim(),
      priority,
      enabled: form.enabled,
      mapping: mappingResult.mapping,
    };
    if (form.token.trim()) body.token = form.token.trim();
    if (provider && form.clearToken) body.clearToken = true;
    save.mutate(body);
  };

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title={provider ? t("Edit {name}", { name: provider.name }) : t("Add metadata provider")}
      description={t(
        "The provider must expose GET /search?title=…&year=… returning a JSON results array. See the contract for the exact shape.",
      )}
      footer={
        <>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            {t("Cancel")}
          </Button>
          <Button variant="primary" type="submit" form="provider-form" loading={save.isPending}>
            {t("Save provider")}
          </Button>
        </>
      }
    >
      <form id="provider-form" onSubmit={submit} className="flex flex-col gap-4">
        {error && (
          <p className="border-l-2 border-danger pl-3 text-[12.5px] leading-relaxed text-danger">{error}</p>
        )}

        <Field label={t("Name")} htmlFor="provider-name" hint={t("Unique name, also used as the stored provider slug.")}>
          <Input
            id="provider-name"
            value={form.name}
            onChange={(event) => set("name", event.target.value)}
            placeholder={t("ScreenScraper")}
            autoComplete="off"
          />
        </Field>

        <Field
          label={t("Base URL")}
          htmlFor="provider-base-url"
          hint={t("HTTPS is recommended. /search and /games/{id} are appended by the server.")}
        >
          <Input
            id="provider-base-url"
            value={form.baseUrl}
            onChange={(event) => set("baseUrl", event.target.value)}
            placeholder="https://metadata.example.com/v1"
            spellCheck={false}
            autoComplete="off"
          />
        </Field>

        <Field
          label={t("Bearer token")}
          htmlFor="provider-token"
          hint={
            provider
              ? t("Leave blank to keep the stored token. It is encrypted at rest and never returned by the API.")
              : t("Optional. Sent as Authorization: Bearer. It is encrypted at rest and never returned by the API.")
          }
        >
          <div className="flex flex-col gap-2">
            {provider?.secretConfigured && (
              <span className="flex h-9 items-center gap-2 text-[13px] text-ink-2">
                <KeyRound className="size-3.5 text-ink-3" aria-hidden="true" />
                {t("A token is stored")}
              </span>
            )}
            <Input
              id="provider-token"
              type="password"
              value={form.token}
              onChange={(event) => {
                set("token", event.target.value);
                if (event.target.value) set("clearToken", false);
              }}
              placeholder={provider ? t("Replace token") : t("Optional token")}
              autoComplete="new-password"
              spellCheck={false}
            />
            {provider?.secretConfigured && (
              <button
                type="button"
                className={form.clearToken ? "self-start text-[12px] text-ok" : "self-start text-[12px] text-danger"}
                onClick={() => {
                  set("clearToken", !form.clearToken);
                  if (!form.clearToken) set("token", "");
                }}
              >
                {form.clearToken ? t("Keep the stored token") : t("Remove the stored token")}
              </button>
            )}
          </div>
        </Field>

        <div className="grid grid-cols-2 gap-4">
          <Field
            label={t("Priority")}
            htmlFor="provider-priority"
            hint={t("Lower runs first. IGDB always runs before every provider.")}
          >
            <Input
              id="provider-priority"
              type="number"
              value={form.priority}
              onChange={(event) => set("priority", event.target.value)}
              min={-1_000_000}
              max={1_000_000}
            />
          </Field>
          <div className="flex items-start justify-between gap-4 border-b border-line pb-2 pt-5">
            <div>
              <p className="text-[13px] font-medium text-ink">{t("Enabled")}</p>
              <p className="mt-0.5 text-[12px] text-ink-3">{t("Run during scans and manual refreshes.")}</p>
            </div>
            <Switch
              checked={form.enabled}
              onCheckedChange={(checked) => set("enabled", checked)}
              label={t("Enable this provider")}
            />
          </div>
        </div>

        <Field
          label={t("Field mapping")}
          htmlFor="provider-mapping"
          hint={t(
            "Optional JSON object mapping canonical fields to dot paths in a non-canonical response, for example {\"title\":\"data.name\"}.",
          )}
        >
          <textarea
            id="provider-mapping"
            value={form.mapping}
            onChange={(event) => set("mapping", event.target.value)}
            rows={6}
            spellCheck={false}
            className="w-full resize-y rounded-[2px] border border-line bg-surface px-3 py-2.5 font-mono text-[12.5px] leading-relaxed text-ink placeholder:text-ink-3"
            placeholder={'{\n  "title": "data.name",\n  "coverUrl": "data.images.cover"\n}'}
          />
        </Field>
        {"error" in mappingResult ? (
          <p className="text-[12px] text-danger">{mappingResult.error}</p>
        ) : (
          <p className="text-[12px] text-ink-3">{t("Canonical fields: {fields}", { fields: CANONICAL_FIELDS.join(", ") })}</p>
        )}
      </form>
    </Dialog>
  );
}

function TestDialog({
  provider,
  onOpenChange,
}: {
  provider: MetadataProvider | null;
  onOpenChange: (open: boolean) => void;
}) {
  const [title, setTitle] = useState("");
  const [year, setYear] = useState("");
  const [result, setResult] = useState<ProviderTestResult | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!provider) return;
    setTitle("");
    setYear("");
    setResult(null);
    setError(null);
  }, [provider]);

  const test = useMutation({
    mutationFn: () =>
      api.testMetadataProvider(provider?.id ?? 0, {
        title: title.trim(),
        year: year.trim() ? Number(year) : undefined,
      }),
    onSuccess: (response) => {
      setResult(response);
      setError(null);
    },
    onError: (cause: Error) => {
      setResult(null);
      setError(errorMessage(cause));
    },
  });

  return (
    <Dialog
      open={provider !== null}
      onOpenChange={onOpenChange}
      title={t("Test {name}", { name: provider?.name ?? t("provider") })}
      description={t("Send a search query and inspect the candidates this provider returns.")}
      footer={
        <>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            {t("Close")}
          </Button>
          <Button
            variant="primary"
            icon={<PlugZap className="size-4" />}
            loading={test.isPending}
            disabled={!title.trim()}
            onClick={() => test.mutate()}
          >
            {t("Run test")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <div className="grid grid-cols-[minmax(0,1fr)_100px] gap-3">
          <Field label={t("Title")} htmlFor="provider-test-title">
            <Input
              id="provider-test-title"
              value={title}
              onChange={(event) => setTitle(event.target.value)}
              placeholder={t("Hades")}
              autoComplete="off"
            />
          </Field>
          <Field label={t("Year")} htmlFor="provider-test-year">
            <Input
              id="provider-test-year"
              type="number"
              value={year}
              onChange={(event) => setYear(event.target.value)}
              min={1900}
              max={2200}
              placeholder="2020"
            />
          </Field>
        </div>

        {error && (
          <p className="border-l-2 border-danger pl-3 text-[12.5px] leading-relaxed text-danger">{error}</p>
        )}

        {result && (
          <section aria-live="polite" className="flex flex-col gap-3">
            <div className="flex flex-wrap items-center gap-2">
              <Badge tone={result.ok ? "ok" : "danger"}>{result.ok ? t("Reachable") : t("Test failed")}</Badge>
              <span className="text-[12px] text-ink-3">
                {t("{count} candidates in {ms} ms", { count: result.candidates.length, ms: result.elapsedMs })}
              </span>
              {result.statusCode !== undefined && (
                <span className="font-mono text-[11px] text-ink-3">HTTP {result.statusCode}</span>
              )}
            </div>
            {result.error && (
              <p className="border-l-2 border-danger pl-3 text-[12.5px] leading-relaxed text-danger">
                {result.error}
              </p>
            )}
            {result.candidates.length > 0 && (
              <ul className="flex flex-col divide-y divide-line border border-line">
                {result.candidates.map((candidate) => (
                  <CandidateRow key={candidate.id} candidate={candidate} />
                ))}
              </ul>
            )}
            {result.ok && result.candidates.length === 0 && (
              <p className="text-[12.5px] text-ink-2">{t("The provider answered but returned no candidate.")}</p>
            )}
          </section>
        )}
      </div>
    </Dialog>
  );
}

function CandidateRow({ candidate }: { candidate: ProviderCandidate }) {
  return (
    <li className="flex items-start gap-3 p-3">
      <div className="min-w-0 flex-1">
        <p className="text-[13px] font-medium text-ink">{candidate.title}</p>
        <p className="mt-0.5 font-mono text-[11px] text-ink-3">
          {candidate.id}
          {candidate.releaseDate ? ` · ${candidate.releaseDate.slice(0, 10)}` : ""}
          {candidate.rating !== undefined ? ` · ${candidate.rating}/100` : ""}
        </p>
        {candidate.description && (
          <p className="mt-1 line-clamp-2 text-[12px] leading-relaxed text-ink-2">{candidate.description}</p>
        )}
      </div>
      {candidate.providerUrl && (
        <a
          href={candidate.providerUrl}
          target="_blank"
          rel="noreferrer"
          className="text-ink-3 hover:text-ink"
          aria-label={t("Open the provider page for {title}", { title: candidate.title })}
        >
          <ExternalLink className="size-3.5" />
        </a>
      )}
    </li>
  );
}
