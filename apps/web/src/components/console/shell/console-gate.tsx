import { useState, type FormEvent } from "react";
import { errorMessage } from "@/lib/http";
import { t } from "@/lib/i18n";
import { useAuth } from "@/state/auth";
import { useSettings } from "@/state/settings";
import { Wordmark } from "@/components/brand";
import { Button, Field, Input } from "@/components/ui/controls";
import { BUILTIN_THEMES, cssVarsFor } from "@/theme/registry";
import {
  DEFAULT_CONSOLE_ROOM,
  cabinetTokens,
  consoleRoomAttributes,
  type ConsoleRoomSettings,
} from "@/theme/console-room";

/**
 * Honest Console gates (#267, #268).
 *
 * Booting is a skeleton the shape of the stage and rail; signed out is a
 * 10-foot sign-in panel on the Cabinet material; no server and offline say so.
 * No spinner, no shimmer, no desktop flash — and the gates wear the Cabinet's
 * default room, because they render before the shell and before the player's
 * own room is known.
 */

/** The Cabinet tokens a gate can render without the shell (#268). */
function useGateMaterial(settings: ConsoleRoomSettings = DEFAULT_CONSOLE_ROOM) {
  const { tokens, resolved } = useSettings();
  const lightTokens = BUILTIN_THEMES.find((theme) => theme.id === "bauhaus")?.tokens ?? tokens;
  const darkTokens = BUILTIN_THEMES.find((theme) => theme.id === "speranza")?.tokens ?? tokens;
  return {
    style: cssVarsFor(
      cabinetTokens({ settings, tokens, lightTokens, darkTokens, mode: resolved }),
    ),
    attributes: consoleRoomAttributes(settings),
  };
}

/** The console's own boot frame: stage + rail silhouette, never a spinner. */
export function ConsoleBoot() {
  const material = useGateMaterial();
  return (
    <div
      className="bp-root relative flex h-dvh flex-col overflow-hidden bg-bg text-ink"
      style={material.style}
      {...material.attributes}
      aria-busy="true"
    >
      <div
        data-bp-bleed-top="true"
        className="relative z-10 flex shrink-0 items-center justify-between border-b bp-chrome bp-pad-x"
      >
        <Wordmark height={28} cut="console" />
        <span className="bp-dot" aria-hidden="true" />
      </div>
      <div className="relative z-10 flex min-h-0 flex-1 flex-col gap-4 p-6 sm:flex-row sm:p-10">
        <div className="flex min-w-0 flex-1 flex-col gap-4">
          <span className="h-[calc(var(--bp-u)*8)] w-32 rounded-full bg-line" />
          <span className="h-[calc(var(--bp-u)*96)] w-full max-w-[640px] rounded-[var(--bp-radius-object)] bg-surface-2/60" />
          <div className="mt-auto flex gap-3">
            <span className="h-[calc(var(--bp-u)*64)] w-28 rounded-[var(--bp-radius-cover)] bg-surface-2/70" />
            <span className="h-[calc(var(--bp-u)*64)] w-28 rounded-[var(--bp-radius-cover)] bg-line/70" />
          </div>
        </div>
        <div className="flex gap-[var(--bp-rail-gap)] sm:w-[42%] sm:max-w-[520px]">
          {[0, 1, 2].map((index) => (
            <span
              key={index}
              className="aspect-[3/4] flex-1 rounded-[var(--bp-radius-cover)] bg-surface-2/50"
            />
          ))}
        </div>
      </div>
      <div className="relative z-10 h-[calc(var(--bp-u)*40)] border-t bp-chrome" />
      <span className="sr-only">{t("Opening the library")}</span>
    </div>
  );
}

function ConsoleGateFrame({ children }: { children: React.ReactNode }) {
  const material = useGateMaterial();
  return (
    <div
      className="bp-root relative grid min-h-dvh place-items-center overflow-hidden bg-bg px-6 text-ink"
      style={material.style}
      {...material.attributes}
    >
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
      <Wordmark height={48} cut="console" className="text-ink" />
      <h1 className="mt-6 text-bp-hero font-semibold leading-[1.02] tracking-[-0.02em]">
        {t("The server did not answer")}
      </h1>
      <p className="mt-3 text-bp-body leading-relaxed text-ink-2">{message}</p>
      <p className="mt-2 text-bp-meta leading-relaxed text-ink-3">
        {t("Check that the Ludex server is running, then try again.")}
      </p>
      <div className="mt-6">
        <Button variant="primary" size="lg" className="bp-button" onClick={retryBoot}>
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
      <Wordmark height={48} cut="console" className="text-ink" />
      <div className="mt-6 flex items-center gap-3">
        <span className="bp-dot" data-tone="muted" aria-hidden="true" />
        <h1 className="text-bp-hero font-semibold leading-[1.02] tracking-[-0.02em]">
          {t("This device is offline.")}
        </h1>
      </div>
      <p className="mt-3 text-bp-body leading-relaxed text-ink-2">
        {t("Check the network, then try again.")}
      </p>
      <div className="mt-6">
        <Button variant="primary" size="lg" className="bp-button" onClick={onRetry}>
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
      <Wordmark height={48} cut="console" className="text-ink" />
      <h1 className="mt-6 text-bp-hero font-semibold leading-[1.02] tracking-[-0.02em]">
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
      <Wordmark height={48} cut="console" className="text-ink" />
      <h1 className="mt-6 text-bp-hero font-semibold leading-[1.02] tracking-[-0.02em]">
        {t("You are signed out.")}
      </h1>
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
            <span className="bp-dot mt-2 shrink-0" aria-hidden="true" />
            {error}
          </p>
        )}
        <div className="flex items-center gap-3">
          <Button type="submit" variant="primary" size="lg" className="bp-button" disabled={busy}>
            {busy ? t("Signing in…") : t("Sign in")}
          </Button>
          <span className="text-bp-meta text-ink-3">{setup?.serverName ?? "Ludex"}</span>
        </div>
      </form>
    </ConsoleGateFrame>
  );
}
