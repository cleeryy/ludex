import { useEffect, useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Check, Copy, ExternalLink, KeyRound, PlugZap, RotateCcw, ShieldCheck } from "lucide-react";
import { api, USING_MOCKS } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import type { OidcAdminResponse, OidcSettings, OidcSettingsInput, OidcTestResult, Role } from "@/lib/types";
import { useToast } from "@/state/toasts";
import { ErrorState, PageHeader, SectionHeader } from "@/components/page";
import { Badge, Button, Field, Input, Skeleton, Switch } from "@/components/ui/controls";
import { Select } from "@/components/ui/select";
import { ConfirmDialog } from "@/components/ui/dialog";
import { formatDateValue, t, tn, tr } from "@/lib/i18n";

const DOCS_URL = "https://github.com/cleeryy/ludex/blob/main/docs/OIDC.md";

function roleOptions(): { value: Role; label: string }[] {
  return [
    { value: "user", label: t("Member") },
    { value: "editor", label: t("Editor") },
    { value: "admin", label: t("Admin") },
  ];
}

interface FormState {
  enabled: boolean;
  issuer: string;
  clientId: string;
  redirectUrl: string;
  publicBaseUrl: string;
  scopes: string;
  displayName: string;
  autoCreate: boolean;
  defaultRole: Role;
  groupsClaim: string;
  adminGroups: string;
}

const PRESETS = [
  {
    id: "authentik",
    label: "Authentik",
    displayName: "Authentik",
    scopes: "openid profile email",
    groupsClaim: "groups",
  },
  {
    id: "keycloak",
    label: "Keycloak",
    displayName: "Keycloak",
    scopes: "openid profile email",
    groupsClaim: "realm_access.roles",
  },
  {
    id: "authelia",
    label: "Authelia",
    displayName: "Authelia",
    scopes: "openid profile email",
    groupsClaim: "groups",
  },
  {
    id: "custom",
    label: t("Custom"),
    displayName: "SSO",
    scopes: "openid profile email",
    groupsClaim: "groups",
  },
] as const;

function toForm(settings: OidcSettings): FormState {
  return {
    enabled: settings.enabled,
    issuer: settings.issuer ?? "",
    clientId: settings.clientId ?? "",
    redirectUrl: settings.redirectUrl ?? "",
    publicBaseUrl: settings.publicBaseUrl ?? "",
    scopes: settings.scopes,
    displayName: settings.displayName,
    autoCreate: settings.autoCreate,
    defaultRole: settings.defaultRole,
    groupsClaim: settings.groupsClaim,
    adminGroups: settings.adminGroups.join("\n"),
  };
}

function splitGroups(value: string): string[] {
  return value
    .split(/[\n,]/)
    .map((group) => group.trim())
    .filter((group, index, all) => group && all.findIndex((entry) => entry.toLowerCase() === group.toLowerCase()) === index);
}

function formatCheckedAt(value: string | null): string {
  if (!value) return t("Not checked yet");
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return formatDateValue(date.toISOString(), {
    day: "numeric",
    month: "short",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

export function AuthenticationScreen() {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const [form, setForm] = useState<FormState | null>(null);
  const [replacingSecret, setReplacingSecret] = useState(false);
  const [replaceSecret, setReplaceSecret] = useState("");
  const [clearSecret, setClearSecret] = useState(false);
  const [testResult, setTestResult] = useState<OidcTestResult | null>(null);
  const [testError, setTestError] = useState<string | null>(null);
  const [resetOpen, setResetOpen] = useState(false);

  const { data, isPending, isError, error, refetch } = useQuery({
    queryKey: ["admin-oidc"],
    queryFn: () => api.adminOidc(),
  });

  useEffect(() => {
    if (data) setForm(toForm(data.settings));
  }, [data]);

  const locked = useMemo(() => new Set(data?.envLockedFields ?? []), [data]);
  const isLocked = (field: string) => locked.has(field);

  const set = <K extends keyof FormState>(key: K, value: FormState[K]) =>
    setForm((current) => (current ? { ...current, [key]: value } : current));

  const payload = (): OidcSettingsInput | null => {
    if (!form) return null;
    return {
      enabled: form.enabled,
      issuer: form.issuer,
      clientId: form.clientId,
      redirectUrl: form.redirectUrl,
      publicBaseUrl: form.publicBaseUrl,
      scopes: form.scopes,
      displayName: form.displayName,
      autoCreate: form.autoCreate,
      defaultRole: form.defaultRole,
      groupsClaim: form.groupsClaim,
      adminGroups: splitGroups(form.adminGroups),
      replaceSecret: replacingSecret && replaceSecret.trim() ? replaceSecret.trim() : null,
      clearSecret,
    };
  };

  const applyResult = (result: OidcAdminResponse) => {
    setForm(toForm(result.settings));
    setReplacingSecret(false);
    setReplaceSecret("");
    setClearSecret(false);
    setTestResult(null);
    setTestError(null);
    queryClient.setQueryData(["admin-oidc"], result);
    void queryClient.invalidateQueries({ queryKey: ["config"] });
  };

  const save = useMutation({
    mutationFn: (body: OidcSettingsInput) => api.saveOidcSettings(body),
    onSuccess: (result) => {
      applyResult(result);
      push({ title: t("OIDC settings applied"), description: t("The login page uses them immediately.") });
    },
    onError: (cause: Error) => {
      setTestError(null);
      void queryClient.invalidateQueries({ queryKey: ["admin-oidc"] });
      push({ title: t("Could not apply the OIDC settings"), description: cause.message, tone: "danger" });
    },
  });

  const test = useMutation({
    mutationFn: (body: OidcSettingsInput) => api.testOidcSettings(body),
    onSuccess: (result) => {
      setTestResult(result);
      setTestError(null);
      void queryClient.invalidateQueries({ queryKey: ["admin-oidc"] });
      push({ title: t("Discovery succeeded"), description: result.discovery.issuer });
    },
    onError: (cause: Error) => {
      setTestResult(null);
      setTestError(cause.message);
      void queryClient.invalidateQueries({ queryKey: ["admin-oidc"] });
    },
  });

  const reset = useMutation({
    mutationFn: () => api.resetOidcSettings(),
    onSuccess: (result) => {
      applyResult(result);
      setResetOpen(false);
      push({ title: t("Environment defaults restored") });
    },
    onError: (cause: Error) => push({ title: t("Could not reset the settings"), description: cause.message, tone: "danger" }),
  });

  const redirectUri = useMemo(() => {
    if (!form) return "";
    const path = form.redirectUrl.trim() || "/api/auth/oidc/callback";
    if (/^https?:\/\//i.test(path)) return path;
    const base = (form.publicBaseUrl.trim() || window.location.origin).replace(/\/$/, "");
    return `${base}${path.startsWith("/") ? path : `/${path}`}`;
  }, [form]);

  const issuerChanged =
    form !== null && data !== undefined && form.issuer.trim() !== (data.settings.issuer ?? "").trim();
  const displayName = form?.displayName.trim() || "SSO";

  const copy = async (value: string, label: string) => {
    try {
      await navigator.clipboard.writeText(value);
      push({ title: t("{label} copied", { label }) });
    } catch {
      push({ title: t("Copy failed"), description: t("Your browser blocked clipboard access."), tone: "danger" });
    }
  };

  const applyPreset = (preset: (typeof PRESETS)[number]) => {
    if (!form) return;
    setForm({
      ...form,
      displayName: isLocked("displayName") ? form.displayName : preset.displayName,
      scopes: isLocked("scopes") ? form.scopes : preset.scopes,
      groupsClaim: isLocked("groupsClaim") ? form.groupsClaim : preset.groupsClaim,
    });
  };

  const status = data?.status;
  const statusTone = status?.checkedAt ? (status.ok ? "ok" : "danger") : "outline";
  const statusLabel = status?.checkedAt ? (status.ok ? t("Reachable") : t("Last check failed")) : t("Not checked yet");

  return (
    <div className="page page-in">
      <PageHeader
        title={t("Authentication")}
        meta={
          <>
            <Badge tone={statusTone}>{statusLabel}</Badge>
            {locked.size > 0 && <span>{tn(locked.size, "1 field set by the environment", "{count} fields set by the environment")}</span>}
            {USING_MOCKS && <Badge tone="outline">{t("Demo data")}</Badge>}
          </>
        }
        description={t("Single sign-on for the web interface. Password sign-in always stays available, and the desktop app keeps using the server password.")}
        actions={
          <>
            <a
              href={DOCS_URL}
              target="_blank"
              rel="noreferrer"
              className="flex h-9 items-center gap-1.5 text-[12.5px] text-ink-2 hover:text-ink"
            >
              
              {t("OIDC guide")}
              <ExternalLink className="size-3.5" aria-hidden="true" />
            </a>
            <Button variant="default" icon={<RotateCcw className="size-4" />} onClick={() => setResetOpen(true)}>
              
              {t("Reset to environment defaults")}
            </Button>
          </>
        }
      />

      {isError && <ErrorState message={errorMessage(error)} onRetry={() => void refetch()} />}

      {isPending && (
        <div className="mt-8 grid gap-10 lg:grid-cols-[minmax(0,1fr)_320px]">
          <div className="flex flex-col gap-4">
            {Array.from({ length: 6 }).map((_, index) => (
              <Skeleton key={index} className="h-11 w-full" />
            ))}
          </div>
          <Skeleton className="h-40 w-full" />
        </div>
      )}

      {!isPending && !isError && form && data && (
        <form
          className="mt-8 grid gap-10 lg:grid-cols-[minmax(0,1fr)_320px]"
          onSubmit={(event) => {
            event.preventDefault();
            const body = payload();
            if (body) save.mutate(body);
          }}
        >
          <div className="flex flex-col gap-9">
            {locked.size > 0 && (
              <p className="border-l-2 border-signal pl-3 text-[12.5px] leading-relaxed text-ink-2">
                
                {tr("Some fields are set by {vars} environment variables and are locked here. Unset those variables and restart the server to manage the values from the UI.", {
                  vars: <span className="font-mono text-ink">LUDEX_OIDC_*</span>,
                })}
              </p>
            )}

            <section>
              <SectionHeader title={t("Provider")} description={t("The issuer URL is the base of the discovery document.")} />
              <div className="mt-4 flex flex-wrap gap-2">
                {PRESETS.map((preset) => (
                  <Button key={preset.id} type="button" variant="default" size="sm" onClick={() => applyPreset(preset)}>
                    {preset.label}
                  </Button>
                ))}
              </div>

              <div className="mt-5 flex flex-col gap-5">
                <div className="flex items-start justify-between gap-6 border-b border-line pb-4">
                  <div>
                    <p className="flex items-center gap-2 text-[13px] font-medium text-ink">
                      
                      {t("Enable SSO")}
                      {isLocked("enabled") && <Badge tone="outline">{t("Environment")}</Badge>}
                    </p>
                    <p className="mt-0.5 text-[12.5px] text-ink-3">
                      
                      {t("Shows “Sign in with {provider}” on the login page. Password sign-in is never disabled.", {
                        provider: displayName,
                      })}
                    </p>
                  </div>
                  <Switch
                    checked={form.enabled}
                    onCheckedChange={(checked) => set("enabled", checked)}
                    label={t("Enable single sign-on")}
                    disabled={isLocked("enabled")}
                  />
                </div>

                <Field
                  label={t("Issuer URL")}
                  htmlFor="oidc-issuer"
                  hint={t("For example https://auth.example.com/application/o/ludex/. HTTPS is required except on localhost.")}
                  aside={isLocked("issuer") ? <Badge tone="outline">{t("Environment")}</Badge> : undefined}
                >
                  <Input
                    id="oidc-issuer"
                    value={form.issuer}
                    onChange={(event) => set("issuer", event.target.value)}
                    placeholder="https://auth.example.com/application/o/ludex/"
                    disabled={isLocked("issuer")}
                    spellCheck={false}
                    autoComplete="off"
                  />
                </Field>

                {issuerChanged && data.linkedAccounts > 0 && (
                  <p className="border-l-2 border-danger pl-3 text-[12.5px] leading-relaxed text-danger">
                    
                    {tn(
                      data.linkedAccounts,
                      "Changing the issuer invalidates the 1 existing SSO link. That account keeps its local password access, but must sign in again with the new provider to be re-linked.",
                      "Changing the issuer invalidates the {count} existing SSO links. Those accounts keep their local password access, but must sign in again with the new provider to be re-linked.",
                    )}
                  </p>
                )}

                <Field
                  label={t("Client ID")}
                  htmlFor="oidc-client-id"
                  aside={isLocked("clientId") ? <Badge tone="outline">{t("Environment")}</Badge> : undefined}
                >
                  <Input
                    id="oidc-client-id"
                    value={form.clientId}
                    onChange={(event) => set("clientId", event.target.value)}
                    placeholder={t("ludex")}
                    disabled={isLocked("clientId")}
                    spellCheck={false}
                    autoComplete="off"
                  />
                </Field>

                <Field
                  label={t("Client secret")}
                  htmlFor="oidc-client-secret"
                  hint={t("Stored encrypted with a key derived from the server secret. It is never shown again; rotating LUDEX_JWT_SECRET requires re-entering it.")}
                  aside={isLocked("clientSecret") ? <Badge tone="outline">{t("Environment")}</Badge> : undefined}
                >
                  <div className="flex flex-col gap-2">
                    <div className="flex flex-wrap items-center gap-2">
                      <span className="flex h-9 flex-1 items-center gap-2 border-b border-line text-[13px] text-ink-2">
                        <KeyRound className="size-3.5 text-ink-3" aria-hidden="true" />
                        {data.settings.secretConfigured ? t("A secret is stored") : t("No secret stored (public client)")}
                      </span>
                      {!isLocked("clientSecret") && (
                        <>
                          <Button
                            type="button"
                            variant="default"
                            size="sm"
                            onClick={() => {
                              setReplacingSecret((current) => !current);
                              setClearSecret(false);
                            }}
                          >
                            {replacingSecret ? t("Keep secret") : t("Replace secret")}
                          </Button>
                          {data.settings.secretConfigured && (
                            <Button
                              type="button"
                              variant="ghost"
                              size="sm"
                              className="text-danger"
                              onClick={() => {
                                setClearSecret((current) => !current);
                                setReplacingSecret(false);
                              }}
                            >
                              {clearSecret ? t("Keep secret") : t("Clear")}
                            </Button>
                          )}
                        </>
                      )}
                    </div>
                    {replacingSecret && !isLocked("clientSecret") && (
                      <Input
                        id="oidc-client-secret"
                        type="password"
                        value={replaceSecret}
                        onChange={(event) => setReplaceSecret(event.target.value)}
                        placeholder={t("New client secret")}
                        autoComplete="new-password"
                        spellCheck={false}
                      />
                    )}
                    {clearSecret && (
                      <p className="text-[12px] text-danger">
                        
                        {t("The stored secret will be removed when you save. Use this only for a public PKCE client.")}
                      </p>
                    )}
                  </div>
                </Field>

                <Field
                  label={t("Public base URL")}
                  htmlFor="oidc-public-base"
                  hint={t("Origin users reach the server on, used to build the redirect URI.")}
                  aside={isLocked("publicBaseUrl") ? <Badge tone="outline">{t("Environment")}</Badge> : undefined}
                >
                  <Input
                    id="oidc-public-base"
                    value={form.publicBaseUrl}
                    onChange={(event) => set("publicBaseUrl", event.target.value)}
                    placeholder="https://ludex.example.com"
                    disabled={isLocked("publicBaseUrl")}
                    spellCheck={false}
                    autoComplete="off"
                  />
                </Field>

                <Field
                  label={t("Redirect URI")}
                  htmlFor="oidc-redirect"
                  hint={t("Register this exact URI at the provider. Leave the field below empty to use the default callback.")}
                  aside={isLocked("redirectUrl") ? <Badge tone="outline">{t("Environment")}</Badge> : undefined}
                >
                  <div className="flex items-center gap-2">
                    <Input id="oidc-redirect" value={form.redirectUrl} onChange={(event) => set("redirectUrl", event.target.value)} placeholder="/api/auth/oidc/callback" disabled={isLocked("redirectUrl")} spellCheck={false} autoComplete="off" />
                  </div>
                  <div className="mt-1 flex items-center gap-2">
                    <code className="truncate font-mono text-[11px] text-ink-2">{redirectUri}</code>
                    <button
                      type="button"
                      onClick={() => void copy(redirectUri, "Redirect URI")}
                      aria-label={t("Copy redirect URI")}
                      className="text-ink-3 hover:text-ink"
                    >
                      <Copy className="size-3.5" />
                    </button>
                  </div>
                </Field>

                <Field label={t("Scopes")} htmlFor="oidc-scopes" aside={isLocked("scopes") ? <Badge tone="outline">{t("Environment")}</Badge> : undefined}>
                  <Input
                    id="oidc-scopes"
                    value={form.scopes}
                    onChange={(event) => set("scopes", event.target.value)}
                    placeholder={t("openid profile email")}
                    disabled={isLocked("scopes")}
                    spellCheck={false}
                    autoComplete="off"
                  />
                </Field>

                <Field
                  label={t("Button label")}
                  htmlFor="oidc-display-name"
                  hint={t("Shown on the login page as “Sign in with …”.")}
                  aside={isLocked("displayName") ? <Badge tone="outline">{t("Environment")}</Badge> : undefined}
                >
                  <Input
                    id="oidc-display-name"
                    value={form.displayName}
                    onChange={(event) => set("displayName", event.target.value)}
                    placeholder={t("Authentik")}
                    disabled={isLocked("displayName")}
                    autoComplete="off"
                  />
                </Field>
              </div>
            </section>

            <section>
              <SectionHeader title={t("Provisioning")} description={t("What happens the first time an identity signs in.")} />
              <div className="mt-4 flex flex-col gap-5">
                <div className="flex items-start justify-between gap-6 border-b border-line pb-4">
                  <div>
                    <p className="flex items-center gap-2 text-[13px] font-medium text-ink">
                      
                      {t("Create accounts automatically")}
                      {isLocked("autoCreate") && <Badge tone="outline">{t("Environment")}</Badge>}
                    </p>
                    <p className="mt-0.5 text-[12.5px] text-ink-3">
                      
                      {t("Unknown identities get a local account on first sign-in. Existing accounts are linked by verified email.")}
                    </p>
                  </div>
                  <Switch
                    checked={form.autoCreate}
                    onCheckedChange={(checked) => set("autoCreate", checked)}
                    label={t("Create accounts automatically")}
                    disabled={isLocked("autoCreate")}
                  />
                </div>

                <Field
                  label={t("Default role")}
                  hint={t("Role for new accounts that are not in an admin group.")}
                  aside={isLocked("defaultRole") ? <Badge tone="outline">{t("Environment")}</Badge> : undefined}
                >
                  <Select<Role>
                    value={form.defaultRole}
                    onChange={(role) => set("defaultRole", role)}
                    options={roleOptions()}
                    label={t("Default role")}
                    disabled={isLocked("defaultRole")}
                  />
                </Field>

                <Field
                  label={t("Groups claim")}
                  htmlFor="oidc-groups-claim"
                  hint={t("Dot paths are allowed, e.g. realm_access.roles for Keycloak.")}
                  aside={isLocked("groupsClaim") ? <Badge tone="outline">{t("Environment")}</Badge> : undefined}
                >
                  <Input
                    id="oidc-groups-claim"
                    value={form.groupsClaim}
                    onChange={(event) => set("groupsClaim", event.target.value)}
                    placeholder={t("groups")}
                    disabled={isLocked("groupsClaim")}
                    spellCheck={false}
                    autoComplete="off"
                  />
                </Field>

                <Field
                  label={t("Admin groups")}
                  htmlFor="oidc-admin-groups"
                  hint={t("One group or role per line (commas work too). Members are granted the admin role on every login.")}
                  aside={isLocked("adminGroups") ? <Badge tone="outline">{t("Environment")}</Badge> : undefined}
                >
                  <textarea
                    id="oidc-admin-groups"
                    value={form.adminGroups}
                    onChange={(event) => set("adminGroups", event.target.value)}
                    disabled={isLocked("adminGroups")}
                    rows={3}
                    spellCheck={false}
                    className="w-full resize-y rounded-[2px] border border-line bg-surface px-3 py-2.5 font-mono text-[12.5px] leading-relaxed text-ink placeholder:text-ink-3 disabled:opacity-40"
                    placeholder={"ludex-admins\noperators"}
                  />
                </Field>
              </div>
            </section>

            <div className="flex flex-wrap items-center gap-3 border-t border-line pt-5">
              <Button type="submit" variant="primary" loading={save.isPending} icon={<ShieldCheck className="size-4" />}>
                {t("Save & apply")}
              </Button>
              <Button
                type="button"
                variant="default"
                icon={<PlugZap className="size-4" />}
                loading={test.isPending}
                onClick={() => {
                  const body = payload();
                  if (body) {
                    test.mutate({ ...body, enabled: true });
                  }
                }}
              >
                
                {t("Test discovery")}
              </Button>
              <span className="text-[12px] text-ink-3">
                
                {t("A failing test or save keeps the current configuration active.")}
              </span>
            </div>
          </div>

          <aside className="flex flex-col gap-5">
            <section className="border border-line bg-surface/50 p-4">
              <div className="flex items-center justify-between gap-2">
                <p className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-3">{t("Status")}</p>
                <Badge tone={statusTone}>{statusLabel}</Badge>
              </div>
              <dl className="mt-3 flex flex-col gap-2 text-[12.5px]">
                <div className="flex items-baseline justify-between gap-3">
                  <dt className="text-ink-3">{t("Last check")}</dt>
                  <dd className="text-ink-2">{formatCheckedAt(status?.checkedAt ?? null)}</dd>
                </div>
                <div className="flex items-baseline justify-between gap-3">
                  <dt className="text-ink-3">{t("Issuer")}</dt>
                  <dd className="truncate font-mono text-[11px] text-ink-2" title={status?.issuer ?? undefined}>
                    {status?.issuer ?? "—"}
                  </dd>
                </div>
                <div className="flex items-baseline justify-between gap-3">
                  <dt className="text-ink-3">{t("Linked accounts")}</dt>
                  <dd className="num text-ink-2">{data.linkedAccounts}</dd>
                </div>
              </dl>
              {status?.error && (
                <p className="mt-3 border-l-2 border-danger pl-2 text-[11.5px] leading-relaxed text-danger">
                  {status.error}
                </p>
              )}
            </section>

            {(testResult || testError) && (
              <section className="border border-line bg-surface/50 p-4" aria-live="polite">
                <p className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-3">{t("Discovery test")}</p>
                {testError && <p className="mt-2 text-[12.5px] leading-relaxed text-danger">{testError}</p>}
                {testResult && (
                  <div className="mt-2 flex flex-col gap-2">
                    <p className="flex items-center gap-1.5 text-[12.5px] text-ok">
                      <Check className="size-3.5" aria-hidden="true" />
                      {testResult.discovery.issuer}
                    </p>
                    {(
                      [
                        ["Authorization", testResult.discovery.authorizationEndpoint],
                        ["Token", testResult.discovery.tokenEndpoint],
                        ["JWKS", testResult.discovery.jwksUri],
                      ] as const
                    ).map(([label, value]) => (
                      <div key={label}>
                        <p className="text-[11px] text-ink-3">{label}</p>
                        <code className="block break-all font-mono text-[10.5px] text-ink-2">{value}</code>
                      </div>
                    ))}
                  </div>
                )}
              </section>
            )}

            <p className="text-[11.5px] leading-relaxed text-ink-3">
              
              {t("The client secret is encrypted at rest and never returned by the API. Saving runs discovery first: if the provider is unreachable, the previous configuration stays active. See the")}{" "}
              <a href={DOCS_URL} target="_blank" rel="noreferrer" className="text-ink-2 underline decoration-line-2 underline-offset-2">
                
                {t("OIDC guide")}
              </a>
              .
            </p>
          </aside>
        </form>
      )}

      <ConfirmDialog
        open={resetOpen}
        onOpenChange={setResetOpen}
        title={t("Reset to environment defaults?")}
        description={t("The saved SSO configuration is deleted and the server falls back to LUDEX_OIDC_* environment variables. With no environment configuration, SSO is disabled. Password sign-in is unaffected.")}
        confirmLabel={t("Reset settings")}
        tone="danger"
        loading={reset.isPending}
        onConfirm={() => reset.mutate()}
      />
    </div>
  );
}
