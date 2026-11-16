// @vitest-environment jsdom
import { act, cleanup, render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SettingsProvider } from "@/state/settings";
import { ToastProvider } from "@/state/toasts";

/**
 * The Console's boot/gate boundary: no desktop login, no desktop toast
 * viewport, and an honest screen for booting, no server and signed out. The
 * routes are stubbed so the test asserts the *shell boundary*, not the library.
 */
const auth = vi.hoisted(() => ({
  value: {
    ready: false,
    bootError: null as string | null,
    user: null as { id: number; username: string } | null,
    setup: { needsSetup: false, serverName: "Ludex", version: "0.4.17" },
    retryBoot: () => undefined,
  },
}));

vi.mock("@/state/auth", () => ({
  useAuth: () => auth.value,
}));

vi.mock("@/components/console/routes", () => ({
  ConsoleRoutes: () => <div data-testid="console-home">home</div>,
}));

import { ConsoleApp } from "@/components/console/console-app";

function renderConsole(ui: React.ReactNode = <ConsoleApp />) {
  // `app.tsx` mounts the console inside AppProviders; the gates read the theme
  // so the Cabinet material is available before the shell exists.
  return render(
    <MemoryRouter initialEntries={["/console"]}>
      <SettingsProvider>{ui}</SettingsProvider>
    </MemoryRouter>,
  );
}

function signedOut() {
  auth.value = { ...auth.value, ready: true, bootError: null, user: null };
}

beforeEach(() => {
  // `motion` and the SettingsProvider read media queries; jsdom has none.
  if (!window.matchMedia) {
    Object.defineProperty(window, "matchMedia", {
      writable: true,
      value: (query: string) => ({
        matches: false,
        media: query,
        onchange: null,
        addListener: () => {},
        removeListener: () => {},
        addEventListener: () => {},
        removeEventListener: () => {},
        dispatchEvent: () => false,
      }),
    });
  }
  window.localStorage.clear();
  auth.value = {
    ready: false,
    bootError: null,
    user: null,
    setup: { needsSetup: false, serverName: "Ludex", version: "0.4.17" },
    retryBoot: () => undefined,
  };
});

afterEach(cleanup);

describe("console boot and gates", () => {
  it("boots as the shape of the stage and rail, without a desktop flash", () => {
    const { container } = renderConsole();
    const root = container.querySelector("[aria-busy='true']");
    expect(root).toBeTruthy();
    // A skeleton, not a spinner and not a shimmer.
    expect(container.querySelector(".animate-spin, [class*='animate-[']")).toBeNull();
    expect(screen.queryByText("The server did not answer")).toBeNull();
    expect(screen.queryByRole("heading")).toBeNull();
  });

  it("shows the no-server screen instead of the desktop login", () => {
    auth.value = { ...auth.value, ready: true, bootError: "connect ECONNREFUSED" };
    renderConsole();
    expect(screen.getByRole("heading", { name: "The server did not answer" })).toBeTruthy();
    expect(screen.queryByText("Sign in")).toBeNull();
  });

  it("gives a signed-out player a console sign-in, with no toast viewport", () => {
    signedOut();
    renderConsole(
      <ToastProvider>
        <ConsoleApp />
      </ToastProvider>,
    );
    expect(screen.getByRole("heading", { name: "You are signed out." })).toBeTruthy();
    expect(screen.getByText("Username")).toBeTruthy();
    expect(document.querySelectorAll("input")).toHaveLength(2);
    // The desktop globals are behind AppShell: no toast region, no modal.
    expect(screen.queryByRole("region", { name: "Notifications" })).toBeNull();
    expect(document.querySelector("[role='dialog']")).toBeNull();
  });

  it("mounts the console routes once ready, and still no desktop globals", () => {
    auth.value = {
      ...auth.value,
      ready: true,
      bootError: null,
      user: { id: 1, username: "tester" },
    };
    renderConsole(
      <ToastProvider>
        <ConsoleApp />
      </ToastProvider>,
    );
    expect(screen.getByTestId("console-home")).toBeTruthy();
    expect(screen.queryByRole("region", { name: "Notifications" })).toBeNull();
    expect(screen.queryByText("Opening the library")).toBeNull();
  });

  it("goes from ready to Home without an intermediate desktop screen", async () => {
    signedOut();
    const { rerender } = renderConsole();
    expect(screen.getByRole("heading", { name: "You are signed out." })).toBeTruthy();

    const started = performance.now();
    auth.value = { ...auth.value, user: { id: 1, username: "tester" } };
    await act(async () => {
      rerender(
        <MemoryRouter initialEntries={["/console"]}>
          <ConsoleApp />
        </MemoryRouter>,
      );
    });
    const elapsed = performance.now() - started;
    expect(screen.getByTestId("console-home")).toBeTruthy();
    // The render path from signed-in state to home is one commit; the bundle
    // transfer is measured by the E2E run, not here. Keep the jsdom figure
    // honest: it must stay far below the 1.2 s boot budget.
    expect(elapsed).toBeLessThan(500);
  });
});
