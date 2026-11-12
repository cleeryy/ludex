import { useState } from "react";
import { Link, useNavigate, useSearchParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { ArrowRight, Loader2, MailOpen } from "lucide-react";
import { api, USING_MOCKS } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { useAuth } from "@/state/auth";
import { AuthLayout } from "@/components/auth-layout";
import { Button, Field, Input } from "@/components/ui/controls";
import { t, tr } from "@/lib/i18n";

/**
 * Invitation landing page (#166): `/register?invite=<token>`.
 *
 * Public on purpose — the invitee has no account yet, so the page must render
 * outside the auth gate. The preview request says whether the link is still
 * usable before the form is shown; redeeming signs the new account in.
 */
export function RegisterScreen() {
  const [params] = useSearchParams();
  const { user, acceptInvitation } = useAuth();
  const navigate = useNavigate();
  const invite = params.get("invite")?.trim() ?? "";

  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const preview = useQuery({
    queryKey: ["invitation", invite],
    queryFn: () => api.invitationPreview(invite),
    enabled: Boolean(invite),
    retry: 0,
    staleTime: 30_000,
  });

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    setError(null);
    if (password.length < 8) {
      setError(t("Use at least eight characters for the password."));
      return;
    }
    if (password !== confirm) {
      setError(t("The two passwords do not match."));
      return;
    }
    setBusy(true);
    try {
      await acceptInvitation(invite, { username: username.trim(), password });
      navigate("/", { replace: true });
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  };

  if (!invite) {
    return (
      <AuthLayout
        statement={t("The shelf you actually own.")}
        note={t("Ludex indexes the archives on your server and serves them to your machines. No storefront, no account anywhere else.")}
      >
        <div className="flex flex-col gap-4">
          <MailOpen className="size-6 text-ink-3" aria-hidden="true" />
          <h1 className="text-[30px] font-semibold leading-tight tracking-[-0.03em]">
            {t("This link is missing its invitation code")}
          </h1>
          <p className="text-[14px] leading-relaxed text-ink-2">
            {t("Ask an administrator for a fresh invitation link, then open it from the beginning.")}
          </p>
          <Link className="text-[13px] text-ink-2 underline decoration-line-2 underline-offset-4 hover:text-ink" to="/login">
            {t("Back to sign in")}
          </Link>
        </div>
      </AuthLayout>
    );
  }

  if (preview.isPending) {
    return (
      <AuthLayout statement={t("The shelf you actually own.")} note={t("One last step, then your library is ready.")}>
        <p className="flex items-center gap-2 text-[13px] text-ink-2" role="status">
          <Loader2 className="size-4 animate-spin" aria-hidden="true" />
          {t("Checking the invitation…")}
        </p>
      </AuthLayout>
    );
  }

  if (preview.isError) {
    return (
      <AuthLayout statement={t("The shelf you actually own.")} note={t("Invitation links expire and only work once.")}>
        <div className="flex flex-col gap-4">
          <MailOpen className="size-6 text-danger" aria-hidden="true" />
          <h1 className="text-[30px] font-semibold leading-tight tracking-[-0.03em]">
            {t("This invitation cannot be used")}
          </h1>
          <p role="alert" className="text-[14px] leading-relaxed text-ink-2">
            {errorMessage(preview.error)}
          </p>
          <p className="text-[12.5px] leading-relaxed text-ink-3">
            {t("An administrator can create a new link in Administration → Invitations.")}
          </p>
          <Link className="text-[13px] text-ink-2 underline decoration-line-2 underline-offset-4 hover:text-ink" to="/login">
            {t("Back to sign in")}
          </Link>
        </div>
      </AuthLayout>
    );
  }

  const invitation = preview.data;
  const expires = new Date(invitation.expiresAt).toLocaleDateString();

  return (
    <AuthLayout
      statement={t("The shelf you actually own.")}
      note={t("Ludex indexes the archives on your server and serves them to your machines. No storefront, no account anywhere else.")}
    >
      <form onSubmit={submit} className="flex flex-col gap-6">
        <div className="flex flex-col gap-2">
          <h1 className="text-[34px] font-semibold leading-none tracking-[-0.03em]">
            {t("Create your account")}
          </h1>
          <p className="text-[13px] leading-relaxed text-ink-3">
            {invitation.email
              ? tr("You were invited as {email}. The link expires on {date}.", {
                  email: <span className="font-mono text-ink-2">{invitation.email}</span>,
                  date: <span className="num">{expires}</span>,
                })
              : tr("Your invitation is valid until {date}.", {
                  date: <span className="num">{expires}</span>,
                })}
          </p>
        </div>

        {invitation.note && (
          <p className="border-l-2 border-line-2 pl-3 text-[12.5px] leading-relaxed text-ink-2">
            {invitation.note}
          </p>
        )}

        {user && (
          <p className="border-l-2 border-signal pl-3 text-[12.5px] leading-relaxed text-ink-2">
            {tr("You are signed in as {user}; redeeming this invitation replaces the current session.", {
              user: <span className="font-mono text-ink">{user.username}</span>,
            })}
          </p>
        )}

        <div className="flex flex-col gap-4">
          <Field label={t("Username")} htmlFor="register-username">
            <Input
              id="register-username"
              name="username"
              autoComplete="username"
              autoFocus
              required
              value={username}
              onChange={(event) => setUsername(event.target.value)}
              placeholder={t("Your username")}
            />
          </Field>
          <Field label={t("Password")} htmlFor="register-password">
            <Input
              id="register-password"
              name="password"
              type="password"
              autoComplete="new-password"
              required
              minLength={8}
              value={password}
              onChange={(event) => setPassword(event.target.value)}
              placeholder={t("At least eight characters")}
            />
          </Field>
          <Field label={t("Confirm password")} htmlFor="register-confirm">
            <Input
              id="register-confirm"
              name="confirm"
              type="password"
              autoComplete="new-password"
              required
              minLength={8}
              value={confirm}
              onChange={(event) => setConfirm(event.target.value)}
            />
          </Field>
        </div>

        {error && (
          <p role="alert" className="border-l-2 border-danger pl-3 text-[13px] text-danger">
            {error}
          </p>
        )}

        <Button
          type="submit"
          variant="primary"
          size="lg"
          loading={busy}
          iconRight={<ArrowRight className="rtl:-scale-x-100 size-4" />}
        >
          {t("Create account and sign in")}
        </Button>

        {USING_MOCKS && (
          <p className="text-[12px] leading-relaxed text-ink-3">
            {t("Demo data is on: this form creates a session for the demo account.")}
          </p>
        )}
      </form>
    </AuthLayout>
  );
}
