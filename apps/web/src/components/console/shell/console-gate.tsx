import { useState, type FormEvent } from "react";
import { AlertTriangle, WifiOff } from "lucide-react";
import { errorMessage } from "@/lib/http";
import { t } from "@/lib/i18n";
import { useAuth } from "@/state/auth";
import { Wordmark } from "@/components/brand";
import { Button, Field, Input } from "@/components/ui/controls";

/**
 * Honest Console gates (#267).
 *
 * The desktop `Gate` redirects a signed-out player to `/login`, which is a
 * desktop screen with a desktop density and a browser sign-in form. A console
 * that is switched on in a living room never shows that: booting is a skeleton
 * the shape of the stage and rail, signed out is a 10-foot sign-in panel on the
 * same material, no server says so, and offline says so. No spinner, no
 * shimmer, no desktop flash.
 */

/** The console's own boot frame: stage + rail silhouette, never a spinner. */
export function ConsoleBoot() {
  return (
    <div className="bp-root relative flex h-dvh flex-col overflow-hidden bg-bg text-ink" aria-busy="true">
      <div className="relative z-10 flex h-[var(--bp-bar-h)] shrink-0 items-center justify-between border-b bp-chrome bp-pad-x">
        <Wordmark height={17} />
        <span className="h-2 w-24 rounded-full bg-line" />
      </div>
      <div className="relative z-10 flex min-h-0 flex-1 flex-col gap-4 p-6 sm:flex-row sm:p-10">
        <div className="flex min-w-0 flex-1 flex-col gap-4">
          <div className="h-3 w-32 rounded-full bg-line" />
          <div className="h-[clamp(120px,18vh,220px)] w-full max-w-[640px] rounded-[var(--bp-radius-md)] bg-surface-2/60" />
          <div className="mt-auto flex gap-3">
            <div className="h-10 w-28 rounded-[var(--bp-radius-sm)] bg-surface-2/70" />
            <div className="h-10 w-28 rounded-[var(--bp-radius-sm)] bg-line/70" />
          </div>
        </div>
        <div className="flex gap-3 sm:w-[42%] sm:max-w-[520px]">
          {[0, 1, 2].map((index) => (
            <div
              key={index}
              className="aspect-[3/4] flex-1 rounded-[var(--bp-radius-sm)] bg-surface-2/50"
            />
          ))}
        </div>
      </div>
      <div className="relative z-10 h-10 border-t bp-chrome" />
      <span className="sr-only">{t("Opening the library")}</span>
    </div>
  );
}

function ConsoleGateFrame({ children }: { children: React.ReactNode }) {
  return (
    <div className="bp-root relative grid min-h-dvh place-items-center overflow-hidden bg-bg px-6 text-ink">
      <div className="grain" aria-hidden="true" />
      <div className="relative z-10 w-full max-w-[520px]">{children}</div>
    </div>
  );
}

/** The server did not answer. The message stays selectable, like the desktop. */
export function ConsoleNoServer({ message }: { message: string }) {
  const { retryBoot } = useAuth();
  return (
    <ConsoleGateFrame>
      <Wordmark height={20} />
      <h1 className="mt-6 text-bp-hero font-semibold leading-tight tracking-[-0.03em]">
        {t("The server did not answer")}
      </h1>
      <p className="mt-3 text-bp-body leading-relaxed text-ink-2">{message}</p>
      <p className="mt-2 text-bp-meta leading-relaxed text-ink-3">
        {t("Check that the Ludex server is running, then try again.")}
      </p>
      <div className="mt-6">
        <Button variant="primary" size="lg" onClick={retryBoot}>
          {t("Try again")}
        </Button>
      </div>
    </ConsoleGateFrame>
  );
}

/** Offline: the console is usable only when it can reach the library. */
export function ConsoleOffline({ onRetry }: { onRetry: () => void }) {
  return (
    <ConsoleGateFrame>
      <Wordmark height={20} />
      <div className="mt-6 flex items-center gap-3">
        <WifiOff className="size-6 text-ink-3" aria-hidden="true" />
        <h1 className="text-bp-hero font-semibold leading-tight tracking-[-0.03em]">{t("This device is offline.")}</h1>
      </div>
      <p className="mt-3 text-bp-body leading-relaxed text-ink-2">{t("Check the network, then try again.")}</p>
      <div className="mt-6">
        <Button variant="primary" size="lg" onClick={onRetry}>
          {t("Try again")}
        </Button>
      </div>
    </ConsoleGateFrame>
  );
}

/** The server has no account yet: setup is a desktop/phone task. */
export function ConsoleNeedsSetup() {
  return (
    <ConsoleGateFrame>
      <Wordmark height={20} />
      <h1 className="mt-6 text-bp-hero font-semibold leading-tight tracking-[-0.03em]">
        {t("Set up Ludex on the server before using the console.")}
      </h1>
      <p className="mt-3 text-bp-body leading-relaxed text-ink-2">
        {t("Open Ludex in a browser on another device to create the first account.")}
      </p>
    </ConsoleGateFrame>
  );
}

/** The console's own sign-in, on console material and at console scale. */
export function ConsoleSignedOut() {
  const { signIn, completeMfaSignIn, setup } = useAuth();
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [challenge, setChallenge] = useState<string | null>(null);
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      if (challenge) {
        await completeMfaSignIn(challenge, code.trim());
      } else {
        const result = await signIn(username.trim(), password);
        if (result.mfaRequired && result.challengeToken) setChallenge(result.challengeToken);
      }
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <ConsoleGateFrame>
      <Wordmark height={20} />
      <h1 className="mt-6 text-bp-hero font-semibold leading-tight tracking-[-0.03em]">{t("You are signed out.")}</h1>
      <p className="mt-2 text-bp-body leading-relaxed text-ink-2">
        {t("Sign in on this device to use the console.")}
      </p>
      <form className="mt-6 flex flex-col gap-4" onSubmit={submit}>
        {challenge ? (
          <Field label={t("Verification code")}>
            <Input
              value={code}
              onChange={(event) => setCode(event.target.value)}
              inputMode="numeric"
              autoComplete="one-time-code"
              autoFocus
            />
          </Field>
        ) : (
          <>
            <Field label={t("Username")}>
              <Input
                value={username}
                onChange={(event) => setUsername(event.target.value)}
                autoComplete="username"
                autoFocus
              />
            </Field>
            <Field label={t("Password")}>
              <Input
                type="password"
                value={password}
                onChange={(event) => setPassword(event.target.value)}
                autoComplete="current-password"
              />
            </Field>
          </>
        )}
        {error && (
          <p role="alert" className="flex items-start gap-2 text-bp-meta leading-relaxed text-danger">
            <AlertTriangle className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
            {error}
          </p>
        )}
        <div className="flex items-center gap-3">
          <Button type="submit" variant="primary" size="lg" disabled={busy}>
            {busy ? t("Signing in…") : t("Sign in")}
          </Button>
          <span className="font-mono text-bp-micro text-ink-3">{setup?.serverName ?? "Ludex"}</span>
        </div>
      </form>
    </ConsoleGateFrame>
  );
}
