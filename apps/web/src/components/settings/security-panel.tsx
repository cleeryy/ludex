import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Copy, KeyRound, Loader2, Monitor, ShieldCheck, ShieldOff } from "lucide-react";
import { api } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { formatDate, formatRelative } from "@/lib/format";
import type { TotpEnrollment } from "@/lib/types";
import { useToast } from "@/state/toasts";
import { Badge, Button, Field, Input } from "@/components/ui/controls";
import { ConfirmDialog } from "@/components/ui/dialog";
import { SettingsGroup } from "@/components/settings/settings-section";
import { t, tr } from "@/lib/i18n";

function CopyButton({ value, label }: { value: string; label: string }) {
  const { push } = useToast();
  return (
    <Button
      variant="quiet"
      size="sm"
      className="px-0 text-ink-2"
      icon={<Copy className="size-3.5" />}
      onClick={() => {
        void navigator.clipboard
          .writeText(value)
          .then(() => push({ title: label, tone: "success" }))
          .catch(() => push({ title: t("Could not copy to the clipboard"), tone: "danger" }));
      }}
    >
      {t("Copy")}
    </Button>
  );
}

/** Recovery codes are shown exactly once; the server only stores their hashes. */
function RecoveryCodeList({ codes }: { codes: string[] }) {
  return (
    <div className="flex flex-col gap-3">
      <div className="grid grid-cols-2 gap-x-4 gap-y-1 font-mono text-[12.5px] text-ink sm:grid-cols-3">
        {codes.map((code) => (
          <span key={code}>{code}</span>
        ))}
      </div>
      <div className="flex flex-wrap gap-4">
        <CopyButton value={codes.join("\n")} label={t("Recovery codes copied")} />
      </div>
    </div>
  );
}

/**
 * Account security (#166): the second factor and the list of signed-in
 * devices. Lives inside Settings → Account so a user never has to hunt for it.
 */
export function SecurityBody() {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const [enrollment, setEnrollment] = useState<TotpEnrollment | null>(null);
  const [code, setCode] = useState("");
  const [recoveryCodes, setRecoveryCodes] = useState<string[] | null>(null);
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [disableOpen, setDisableOpen] = useState(false);
  const [revokeOpen, setRevokeOpen] = useState(false);

  const mfa = useQuery({ queryKey: ["mfa"], queryFn: () => api.mfaStatus() });
  const sessions = useQuery({ queryKey: ["sessions"], queryFn: () => api.listSessions() });

  const refreshMfa = () => void queryClient.invalidateQueries({ queryKey: ["mfa"] });

  const enroll = useMutation({
    mutationFn: () => api.startTotpEnrollment(),
    onSuccess: (data) => {
      setEnrollment(data);
      setRecoveryCodes(null);
      setCode("");
      setError(null);
    },
    onError: (cause: Error) => setError(cause.message),
  });

  const activate = useMutation({
    mutationFn: (value: string) => api.activateTotp(value),
    onSuccess: (data) => {
      setRecoveryCodes(data.recoveryCodes);
      setEnrollment(null);
      setCode("");
      refreshMfa();
      push({ title: t("Two-factor authentication enabled"), tone: "success" });
    },
    onError: (cause: Error) => setError(cause.message),
  });

  const regenerate = useMutation({
    mutationFn: () => api.regenerateRecoveryCodes(password),
    onSuccess: (data) => {
      setRecoveryCodes(data.codes);
      setPassword("");
      refreshMfa();
      push({ title: t("New recovery codes generated"), tone: "success" });
    },
    onError: (cause: Error) => setError(cause.message),
  });

  const disable = useMutation({
    mutationFn: () => api.disableMfa(password),
    onSuccess: () => {
      setPassword("");
      setDisableOpen(false);
      setRecoveryCodes(null);
      refreshMfa();
      void queryClient.invalidateQueries({ queryKey: ["sessions"] });
      push({ title: t("Two-factor authentication disabled"), tone: "success" });
    },
    onError: (cause: Error) => {
      setDisableOpen(false);
      setError(cause.message);
    },
  });

  const revokeOne = useMutation({
    mutationFn: (id: number) => api.revokeSession(id),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["sessions"] });
      push({ title: t("Session signed out"), tone: "success" });
    },
    onError: (cause: Error) => push({ title: t("Could not revoke the session"), description: errorMessage(cause), tone: "danger" }),
  });

  const revokeOthers = useMutation({
    mutationFn: () => api.revokeOtherSessions(),
    onSuccess: (data) => {
      setRevokeOpen(false);
      void queryClient.invalidateQueries({ queryKey: ["sessions"] });
      push({
        title: tnRevoked(data.revoked),
        tone: "success",
      });
    },
    onError: (cause: Error) => {
      setRevokeOpen(false);
      push({ title: t("Could not revoke the sessions"), description: errorMessage(cause), tone: "danger" });
    },
  });

  const status = mfa.data;
  const active = status?.enabled ?? false;
  const otherSessions = (sessions.data ?? []).filter((session) => !session.current && !session.revokedAt);

  return (
    <>
      <SettingsGroup
        title={t("Two-factor authentication")}
        description={t("A six-digit code from an authenticator app is required after your password.")}
        actions={
          active ? (
            <Badge tone="ok">{t("On")}</Badge>
          ) : (
            <Badge tone="outline">{t("Off")}</Badge>
          )
        }
      >
        <div className="flex max-w-[560px] flex-col gap-4">
          {mfa.isPending && (
            <p className="flex items-center gap-2 text-[12.5px] text-ink-3" role="status">
              <Loader2 className="size-3.5 animate-spin" aria-hidden="true" />
              {t("Loading…")}
            </p>
          )}

          {active && !recoveryCodes && (
            <>
              <p className="text-[13px] leading-relaxed text-ink-2">
                {tr("Enabled on {date}. {count} recovery codes left.", {
                  date: <span className="num">{status?.enabledAt ? formatDate(status.enabledAt) : "—"}</span>,
                  count: <span className="num">{status?.recoveryCodesRemaining ?? 0}</span>,
                })}
              </p>
              <div className="flex flex-col gap-3">
                <Field label={t("Current password")} htmlFor="mfa-regenerate-password">
                  <Input
                    id="mfa-regenerate-password"
                    type="password"
                    autoComplete="current-password"
                    value={password}
                    onChange={(event) => setPassword(event.target.value)}
                  />
                </Field>
                <div className="flex flex-wrap gap-2">
                  <Button
                    variant="default"
                    icon={<KeyRound className="size-4" />}
                    loading={regenerate.isPending}
                    disabled={!password}
                    onClick={() => {
                      setError(null);
                      regenerate.mutate();
                    }}
                  >
                    {t("New recovery codes")}
                  </Button>
                  <Button
                    variant="ghost"
                    icon={<ShieldOff className="size-4" />}
                    disabled={!password}
                    onClick={() => setDisableOpen(true)}
                  >
                    {t("Disable")}
                  </Button>
                </div>
                <p className="text-[11.5px] leading-relaxed text-ink-3">
                  {t("Generating new recovery codes invalidates the previous set. Disabling signs out your other devices.")}
                </p>
              </div>
            </>
          )}

          {!active && !enrollment && (
            <>
              <p className="text-[13px] leading-relaxed text-ink-2">
                {t("Use any TOTP app: 1Password, Bitwarden, Aegis, Google Authenticator. Ludex shows the secret once.")}
              </p>
              <Button
                variant="default"
                icon={<ShieldCheck className="size-4" />}
                loading={enroll.isPending}
                onClick={() => {
                  setError(null);
                  enroll.mutate();
                }}
              >
                {t("Set up an authenticator")}
              </Button>
            </>
          )}

          {enrollment && (
            <div className="flex flex-col gap-3">
              <p className="text-[12.5px] leading-relaxed text-ink-3">
                {t("Add this key to your authenticator app, then enter the code it shows. The key is not shown again.")}
              </p>
              <div className="flex flex-wrap items-center gap-3">
                <code className="break-all border border-line-2 bg-surface px-3 py-2 font-mono text-[13px] tracking-[0.12em] text-ink">
                  {enrollment.secret}
                </code>
                <CopyButton value={enrollment.secret} label={t("Secret copied")} />
              </div>
              <p className="break-all font-mono text-[11px] leading-relaxed text-ink-3">
                {enrollment.otpauthUri}
              </p>
              <Field label={t("Six-digit code")} htmlFor="mfa-activate-code">
                <Input
                  id="mfa-activate-code"
                  autoComplete="one-time-code"
                  inputMode="numeric"
                  maxLength={7}
                  value={code}
                  onChange={(event) => setCode(event.target.value)}
                  placeholder="123456"
                />
              </Field>
              <div className="flex flex-wrap gap-2">
                <Button
                  variant="primary"
                  loading={activate.isPending}
                  disabled={code.trim().length < 6}
                  onClick={() => {
                    setError(null);
                    activate.mutate(code.trim());
                  }}
                >
                  {t("Turn on two-factor authentication")}
                </Button>
                <Button variant="ghost" onClick={() => setEnrollment(null)}>
                  {t("Cancel")}
                </Button>
              </div>
            </div>
          )}

          {recoveryCodes && (
            <div className="flex flex-col gap-3 border border-line-2 p-4">
              <p className="text-[13px] font-medium text-ink">{t("Save your recovery codes now")}</p>
              <p className="text-[12px] leading-relaxed text-ink-3">
                {t("Each code signs you in once if you lose the authenticator. They are hashed on the server and cannot be shown again.")}
              </p>
              <RecoveryCodeList codes={recoveryCodes} />
              <Button variant="default" size="sm" className="self-start" onClick={() => setRecoveryCodes(null)}>
                {t("I saved them")}
              </Button>
            </div>
          )}

          {error && (
            <p role="alert" className="border-l-2 border-danger pl-3 text-[12.5px] text-danger">
              {error}
            </p>
          )}
        </div>
      </SettingsGroup>

      <SettingsGroup
        title={t("Active sessions")}
        description={t("Browsers and desktop apps signed in to this account. Revoke anything you do not recognise.")}
        actions={
          otherSessions.length > 0 ? (
            <Button variant="ghost" size="sm" onClick={() => setRevokeOpen(true)}>
              {tnOthers(otherSessions.length)}
            </Button>
          ) : undefined
        }
      >
        <div className="flex flex-col divide-y divide-line border-y border-line">
          {sessions.isPending && (
            <p className="flex items-center gap-2 py-3 text-[12.5px] text-ink-3" role="status">
              <Loader2 className="size-3.5 animate-spin" aria-hidden="true" />
              {t("Loading…")}
            </p>
          )}
          {(sessions.data ?? []).map((session) => (
            <div key={session.id} className="flex flex-wrap items-center justify-between gap-3 py-3">
              <div className="flex min-w-0 items-start gap-3">
                <Monitor className="mt-0.5 size-4 shrink-0 text-ink-3" aria-hidden="true" />
                <div className="min-w-0">
                  <p className="flex flex-wrap items-center gap-2 text-[13px] text-ink">
                    {session.device ?? session.userAgent ?? t("Unknown device")}
                    {session.current && <Badge tone="signal">{t("This session")}</Badge>}
                    {session.revokedAt && <Badge tone="danger">{t("Revoked")}</Badge>}
                  </p>
                  <p className="mt-0.5 text-[11.5px] text-ink-3">
                    <span className="num">{session.ip ?? "—"}</span>
                    <span className="px-1.5">·</span>
                    {tr("last seen {date}", {
                      date: <span className="num">{formatRelative(session.lastSeenAt)}</span>,
                    })}
                  </p>
                </div>
              </div>
              {!session.revokedAt && (
                <Button
                  variant="ghost"
                  size="sm"
                  loading={revokeOne.isPending && revokeOne.variables === session.id}
                  onClick={() => revokeOne.mutate(session.id)}
                >
                  {session.current ? t("Sign out") : t("Revoke")}
                </Button>
              )}
            </div>
          ))}
          {sessions.isSuccess && (sessions.data ?? []).length === 0 && (
            <p className="py-3 text-[12.5px] text-ink-3">{t("No sessions recorded.")}</p>
          )}
        </div>
      </SettingsGroup>

      <ConfirmDialog
        open={disableOpen}
        onOpenChange={setDisableOpen}
        title={t("Disable two-factor authentication?")}
        description={t("Your password alone will sign you in again. Your other devices are signed out.")}
        confirmLabel={t("Disable")}
        loading={disable.isPending}
        onConfirm={() => {
          setError(null);
          disable.mutate();
        }}
      />

      <ConfirmDialog
        open={revokeOpen}
        onOpenChange={setRevokeOpen}
        title={tnOthers(otherSessions.length)}
        description={t("Those devices will need to sign in again. This session stays active.")}
        confirmLabel={t("Sign out other devices")}
        loading={revokeOthers.isPending}
        onConfirm={() => revokeOthers.mutate()}
      />
    </>
  );
}

/** Pluralised "Sign out N other devices" used as a button and dialog title. */
function tnOthers(count: number): string {
  return count === 1
    ? t("Sign out the other device")
    : t("Sign out {count} other devices", { count });
}

function tnRevoked(count: number): string {
  return count === 1
    ? t("Signed out 1 session")
    : t("Signed out {count} sessions", { count });
}
