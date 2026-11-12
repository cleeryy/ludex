import { describe, expect, it } from "vitest";
import en from "../locales/en.json";
import {
  providerName,
  setupAuthOptions,
  setupHandoffBlocker,
  setupHandoffView,
} from "./setup-auth";

/**
 * First-run setup choices (#175).
 *
 * These helpers decide what the desktop wizard and the browser setup page
 * render: the SSO option exists only when the server has a provider, the local
 * administrator always remains the fallback, and the handoff panel has one
 * state per phase (idle, progress, error) with the right escape hatches.
 */

describe("setup auth options (#175)", () => {
  it("offers the provider first for a server with no users", () => {
    const options = setupAuthOptions({ needsSetup: true, oidcEnabled: true, displayName: "Authentik" });
    expect(options.map((option) => option.method)).toEqual(["sso", "local-admin"]);
    expect(options[0]).toMatchObject({
      label: "Sign in with Authentik",
      primary: true,
    });
    expect(options[1]).toMatchObject({ label: "Create the administrator", primary: false });
  });

  it("keeps the local administrator when the server has no provider", () => {
    const options = setupAuthOptions({ needsSetup: true, oidcEnabled: false });
    expect(options.map((option) => option.method)).toEqual(["local-admin"]);
    expect(options[0].primary).toBe(true);
    expect(options[0].description).toMatch(/first account/i);
  });

  it("uses the SSO option for a server that already has users", () => {
    const options = setupAuthOptions({ needsSetup: false, oidcEnabled: true, displayName: "Keycloak" });
    expect(options.map((option) => option.method)).toEqual(["sso", "password"]);
    expect(options[0].label).toBe("Sign in with Keycloak");
    expect(options[0].primary).toBe(true);
    expect(options[1].label).toBe("Sign in here instead");
    expect(options[1].primary).toBe(false);
  });

  it("never leaves the password form without a primary action", () => {
    const options = setupAuthOptions({ needsSetup: false, oidcEnabled: false });
    expect(options).toHaveLength(1);
    expect(options[0]).toMatchObject({ method: "password", primary: true });
  });

  it("falls back to the generic SSO label for a blank display name", () => {
    expect(providerName("  ")).toBe("SSO");
    expect(providerName(null)).toBe("SSO");
    expect(providerName("Authelia")).toBe("Authelia");
    const options = setupAuthOptions({ needsSetup: true, oidcEnabled: true, displayName: null });
    expect(options[0].label).toBe("Sign in with SSO");
  });

  it("only ever marks one option primary", () => {
    for (const needsSetup of [true, false]) {
      for (const oidcEnabled of [true, false]) {
        const options = setupAuthOptions({ needsSetup, oidcEnabled, displayName: "Authentik" });
        expect(options.filter((option) => option.primary)).toHaveLength(1);
      }
    }
  });

  it("keeps every non-composed label and description in the English catalog", () => {
    for (const needsSetup of [true, false]) {
      for (const oidcEnabled of [true, false]) {
        for (const option of setupAuthOptions({ needsSetup, oidcEnabled })) {
          expect(en).toHaveProperty(option.description);
          if (option.method !== "sso") expect(en).toHaveProperty(option.label);
        }
      }
    }
  });
});

describe("setup handoff eligibility (#175)", () => {
  const ready = {
    native: true,
    oidcEnabled: true,
    serverUrl: "http://10.0.0.5:8080",
    phase: "idle" as const,
  };

  it("allows the desktop app to start against a server with a provider", () => {
    expect(setupHandoffBlocker(ready)).toBeNull();
  });

  it("refuses outside the desktop shell", () => {
    expect(setupHandoffBlocker({ ...ready, native: false })).toBe("not-desktop");
  });

  it("refuses without a configured provider", () => {
    expect(setupHandoffBlocker({ ...ready, oidcEnabled: false })).toBe("no-provider");
  });

  it("refuses without a server address", () => {
    expect(setupHandoffBlocker({ ...ready, serverUrl: "" })).toBe("missing-address");
    expect(setupHandoffBlocker({ ...ready, serverUrl: "   " })).toBe("missing-address");
    expect(setupHandoffBlocker({ ...ready, serverUrl: null })).toBe("missing-address");
  });

  it("refuses a second start while one is in flight or waiting", () => {
    expect(setupHandoffBlocker({ ...ready, phase: "starting" })).toBe("in-progress");
    expect(setupHandoffBlocker({ ...ready, phase: "waiting" })).toBe("in-progress");
  });

  it("allows a retry after an error", () => {
    expect(setupHandoffBlocker({ ...ready, phase: "error" })).toBeNull();
  });
});

describe("setup handoff view (#175)", () => {
  it("shows nothing before the first start", () => {
    expect(setupHandoffView({ phase: "idle" })).toEqual({
      progress: null,
      error: null,
      canCancel: false,
      canRetry: false,
      canOpenWebLogin: false,
    });
  });

  it("shows progress and a cancel while the browser opens", () => {
    const view = setupHandoffView({ phase: "starting" });
    expect(view.progress).toBe("Opening your browser…");
    expect(view.canCancel).toBe(true);
    expect(view.canRetry).toBe(false);
    expect(view.error).toBeNull();
  });

  it("offers the web login fallback while waiting for the browser", () => {
    const view = setupHandoffView({
      phase: "waiting",
      webLoginUrl: "http://10.0.0.5:8080/#/login?desktop_state=abc",
    });
    expect(view.progress).toBe("Waiting for your browser");
    expect(view.canCancel).toBe(true);
    expect(view.canOpenWebLogin).toBe(true);
    expect(view.error).toBeNull();
  });

  it("shows the error with retry and the web login fallback", () => {
    const view = setupHandoffView({
      phase: "error",
      message: "The sign-in window expired.",
      webLoginUrl: null,
    });
    expect(view.progress).toBeNull();
    expect(view.error).toBe("The sign-in window expired.");
    expect(view.canRetry).toBe(true);
    expect(view.canCancel).toBe(false);
    // A failed start has no claim URL, but the plain web login can still open.
    expect(view.canOpenWebLogin).toBe(true);
  });

  it("has a default error when the poll returns no message", () => {
    const view = setupHandoffView({ phase: "error", message: "   " });
    expect(view.error).toBe("The browser sign-in did not finish. Try again.");
  });
});
