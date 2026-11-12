import { t } from "@/lib/i18n";

/**
 * First-run setup choices (#175).
 *
 * The desktop wizard can reach two kinds of server: one with no users yet
 * (`needsSetup`) and one that already has accounts. Both can sign in through
 * the browser handoff (#157) when the server has an OIDC provider, but only a
 * server with no users also needs — and can create — a local administrator.
 * Keeping the ordering and the eligibility rules here means the wizard, the
 * browser setup page and the tests agree, and keeps the JSX free of branching.
 */

/** What a setup screen can offer. */
export type SetupAuthMethod = "sso" | "local-admin" | "password";

export interface SetupAuthOption {
  method: SetupAuthMethod;
  /** Button label, already translated. */
  label: string;
  /** One-line explanation, already translated. */
  description: string;
  /** The action rendered as primary; exactly one option sets it. */
  primary: boolean;
}

/** Label shown after "Sign in with"; the provider name, or the generic SSO. */
export function providerName(displayName?: string | null): string {
  return displayName?.trim() || "SSO";
}

/**
 * Ordered sign-in choices for a first-run or connect screen.
 *
 * SSO comes first when the server has a provider because it is the only option
 * for an identity that has no local password. Without a provider the local
 * administrator (no users yet) or the password form (existing server) is the
 * only choice and becomes primary.
 */
export function setupAuthOptions(input: {
  /** The server has no users yet (`needsSetup` from `/api/setup/status`). */
  needsSetup: boolean;
  /** `features.oidc` from `/api/config`. */
  oidcEnabled: boolean;
  /** `oidcDisplayName` from `/api/config`; blank falls back to "SSO". */
  displayName?: string | null;
}): SetupAuthOption[] {
  const options: SetupAuthOption[] = [];
  if (input.oidcEnabled) {
    options.push({
      method: "sso",
      label: `${t("Sign in with")} ${providerName(input.displayName)}`,
      description: input.needsSetup
        ? t("Opens the server login in your browser. The provider completes the setup when it may create accounts.")
        : t("Opens the server login in your browser and brings the session back to this app."),
      primary: true,
    });
  }
  if (input.needsSetup) {
    options.push({
      method: "local-admin",
      label: t("Create the administrator"),
      description: t("Creates the first account on this server. It stays on that machine."),
      primary: !input.oidcEnabled,
    });
  } else {
    options.push({
      method: "password",
      label: t("Sign in here instead"),
      description: t("Use a username and password issued by this server."),
      primary: !input.oidcEnabled,
    });
  }
  return options;
}

/** Progress of one browser handoff attempt started from a setup screen. */
export type SetupHandoffPhase = "idle" | "starting" | "waiting" | "error";

/** Why a setup screen cannot start the handoff right now. */
export type SetupHandoffBlocker = "not-desktop" | "no-provider" | "missing-address" | "in-progress";

/**
 * Whether the first-run screen may start the browser handoff (#175).
 *
 * The handoff is desktop-only: a browser first run creates the administrator in
 * place, and a server without a provider has nothing to hand off. A retry is
 * allowed right after an error, a second start is not.
 */
export function setupHandoffBlocker(input: {
  /** `isNative()`: only the Tauri shell can open a browser and poll. */
  native: boolean;
  oidcEnabled: boolean;
  /** The candidate server address, before it is written to the settings. */
  serverUrl: string | null | undefined;
  phase: SetupHandoffPhase;
}): SetupHandoffBlocker | null {
  if (!input.native) return "not-desktop";
  if (!input.oidcEnabled) return "no-provider";
  if (!input.serverUrl?.trim()) return "missing-address";
  if (input.phase === "starting" || input.phase === "waiting") return "in-progress";
  return null;
}

/** Presentation state of the handoff panel; `null` means "hide it". */
export interface SetupHandoffView {
  /** Progress copy while the browser is being opened or is being waited on. */
  progress: string | null;
  error: string | null;
  canCancel: boolean;
  canRetry: boolean;
  /** "Open web login": the browser page can be reopened without a new poll. */
  canOpenWebLogin: boolean;
}

/**
 * Turns the raw attempt state into the four things the panel shows: progress,
 * error, cancel/retry and the "Open web login" fallback. Before the first start
 * there is nothing to show, and before a URL exists the fallback has no target
 * unless the attempt already failed (the panel then opens `/#/login`).
 */
export function setupHandoffView(input: {
  phase: SetupHandoffPhase;
  message?: string | null;
  webLoginUrl?: string | null;
}): SetupHandoffView {
  const progress =
    input.phase === "starting"
      ? t("Opening your browser…")
      : input.phase === "waiting"
        ? t("Waiting for your browser")
        : null;
  return {
    progress,
    error:
      input.phase === "error"
        ? input.message?.trim() || t("The browser sign-in did not finish. Try again.")
        : null,
    canCancel: input.phase === "starting" || input.phase === "waiting",
    canRetry: input.phase === "error",
    canOpenWebLogin:
      input.phase === "error" || Boolean(input.webLoginUrl?.trim()),
  };
}
