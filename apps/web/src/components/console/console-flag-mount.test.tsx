// @vitest-environment jsdom
import { act, cleanup, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider, useQuery } from "@tanstack/react-query";
import { useEffect } from "react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { setConsoleShellEnabled } from "@/lib/console-flag";

/**
 * `console_v2` changes the entry, not the mount (#304).
 *
 * The flag is read through `useSyncExternalStore`, so flipping it while the app
 * is running re-renders the gate. The risk was never the re-render: it was a
 * second library shell mounting below the first when the redirect ran. This
 * test mounts the real `Gate` and a real router, flips the device flag
 * on -> off -> on, and counts mounts plus actual query executions.
 *
 * The library stub stands in for the desktop/console route bodies; the router
 * decision, the flag store and the redirect are the production modules.
 */

const auth = vi.hoisted(() => ({
  value: {
    ready: true,
    bootError: null as string | null,
    user: { id: 1, username: "tester" } as { id: number; username: string } | null,
    setup: { needsSetup: false, serverName: "Ludex", version: "0.4.24" },
    holdLogin: false,
    retryBoot: () => undefined,
  },
}));

vi.mock("@/state/auth", () => ({
  useAuth: () => auth.value,
  useIsAdmin: () => true,
  useIsEditor: () => true,
  AuthProvider: ({ children }: { children: React.ReactNode }) => children,
}));

import { Gate } from "@/app";

const counters = { desktop: 0, console: 0, queries: 0 };

function DesktopLibrary() {
  useEffect(() => {
    counters.desktop += 1;
  }, []);
  return <div data-testid="desktop-library">desktop</div>;
}

function ConsoleLibrary() {
  useEffect(() => {
    counters.console += 1;
  }, []);
  // The console's library is one query set. A second mount would run it again.
  useQuery({
    queryKey: ["e2e", "console-library"],
    queryFn: async () => {
      counters.queries += 1;
      return [];
    },
    staleTime: Number.POSITIVE_INFINITY,
  });
  return <div data-testid="console-library">console</div>;
}

function renderApp() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={["/"]}>
        <Routes>
          <Route
            path="/"
            element={
              <Gate>
                <DesktopLibrary />
              </Gate>
            }
          />
          <Route path="/console/*" element={<ConsoleLibrary />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
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
  setConsoleShellEnabled(false);
  counters.desktop = 0;
  counters.console = 0;
  counters.queries = 0;
  auth.value = { ...auth.value, ready: true, bootError: null, user: { id: 1, username: "tester" } };
});

afterEach(cleanup);

describe("console_v2 flag mount discipline", () => {
  it("on -> off -> on mounts one console library and runs one query set", async () => {
    renderApp();
    expect(screen.getByTestId("desktop-library")).toBeTruthy();

    // On: the root redirects to /console and the console mounts once.
    await act(async () => {
      setConsoleShellEnabled(true);
    });
    expect(await screen.findByTestId("console-library")).toBeTruthy();
    expect(counters.console).toBe(1);

    // Off while the player is inside /console: the Console is still a route (a
    // deep link is always allowed), so nothing is torn down or re-mounted.
    await act(async () => {
      setConsoleShellEnabled(false);
    });
    expect(screen.getByTestId("console-library")).toBeTruthy();

    // On again: same mount, same query. A second shell would show up here.
    await act(async () => {
      setConsoleShellEnabled(true);
    });
    expect(screen.getByTestId("console-library")).toBeTruthy();

    expect(counters.desktop).toBe(1);
    expect(counters.console).toBe(1);
    expect(counters.queries).toBe(1);
  });
});
