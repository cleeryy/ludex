// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { CacheEntryView, CacheOverview, ServerConfig } from "@/lib/types";

const mocks = vi.hoisted(() => ({
  extractedCache: vi.fn(),
  evictExtractedCache: vi.fn(),
  config: vi.fn(),
  push: vi.fn(),
}));

vi.mock("@/lib/api", () => ({
  api: {
    extractedCache: (...args: unknown[]) => mocks.extractedCache(...args),
    evictExtractedCache: (...args: unknown[]) => mocks.evictExtractedCache(...args),
    config: (...args: unknown[]) => mocks.config(...args),
  },
}));

vi.mock("@/state/toasts", () => ({
  useToast: () => ({ push: mocks.push }),
}));

import { ApiError } from "@/lib/http";
import { ExtractedCacheScreen } from "@/screens/admin/cache";

beforeAll(() => {
  // `motion` reads the reduced-motion media query when a dialog opens; jsdom
  // has no implementation.
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
});

afterEach(() => {
  cleanup();
});

beforeEach(() => {
  mocks.extractedCache.mockReset();
  mocks.evictExtractedCache.mockReset();
  mocks.config.mockReset();
  mocks.push.mockReset();
  mocks.config.mockResolvedValue(serverConfig(true));
});

function serverConfig(localMode: boolean): ServerConfig {
  return {
    serverName: "Ludex",
    version: "0.4.19",
    features: { igdb: false, steam: false, ingest: false, localMode, oidc: false },
    oidcDisplayName: null,
    maxUploadBytes: 1,
    defaultLocale: "",
  };
}

const SHA_ONE = "a".repeat(64);
const SHA_TWO = "0123456789abcdef".repeat(4);

function entry(patch: Partial<CacheEntryView> = {}): CacheEntryView {
  return {
    sha256: SHA_ONE,
    versionId: 42,
    bytes: 5 * 1024 ** 3,
    files: 1_240,
    createdAt: "2026-11-01T10:00:00Z",
    lastUsedAt: "2026-11-15T09:30:00Z",
    ...patch,
  };
}

function overview(patch: Partial<CacheOverview> = {}): CacheOverview {
  return {
    enabled: true,
    root: "/srv/ludex/data/extracted-cache",
    budgetBytes: 50 * 1024 ** 3,
    usedBytes: 5 * 1024 ** 3,
    entries: [entry()],
    ...patch,
  };
}

function renderScreen() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter>
        <ExtractedCacheScreen />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("extracted cache screen", () => {
  it("lists entries with a short digest, the full digest on hover, and the budget", async () => {
    mocks.extractedCache.mockResolvedValue(overview());
    renderScreen();

    expect(await screen.findByText("aaaaaaaaaaaa…")).toBeTruthy();
    expect(screen.getByTitle(SHA_ONE)).toBeTruthy();
    expect(screen.getByText("#42")).toBeTruthy();
    expect(screen.getAllByText("5.0 GB").length).toBeGreaterThan(1);
    expect(screen.getByText("1,240")).toBeTruthy();
    expect(screen.getByText("/srv/ludex/data/extracted-cache")).toBeTruthy();
    expect(screen.getByText("10% of the budget")).toBeTruthy();
  });

  it("explains the off state and points local mode at the download settings", async () => {
    mocks.extractedCache.mockResolvedValue(overview({ enabled: false, usedBytes: 0, entries: [] }));
    renderScreen();

    expect(await screen.findByText("The extracted cache is off")).toBeTruthy();
    const link = await screen.findByRole("link", { name: /Open download settings/ });
    expect(link.getAttribute("href")).toBe("/settings?section=downloads");
    expect(screen.queryByText("Capacity")).toBeNull();
  });

  it("tells a self-hosted operator which environment variable turns the cache on", async () => {
    mocks.extractedCache.mockResolvedValue(overview({ enabled: false, usedBytes: 0, entries: [] }));
    mocks.config.mockResolvedValue(serverConfig(false));
    renderScreen();

    expect(await screen.findByText(/LUDEX_EXTRACTED_CACHE=1/)).toBeTruthy();
    expect(screen.queryByRole("link", { name: /Open download settings/ })).toBeNull();
  });

  it("evicts the confirmed entry and refreshes the overview", async () => {
    let entries = [entry(), entry({ sha256: SHA_TWO, versionId: 43, bytes: 2 * 1024 ** 3 })];
    mocks.extractedCache.mockImplementation(async () =>
      overview({
        usedBytes: entries.reduce((sum, item) => sum + item.bytes, 0),
        entries,
      }),
    );
    mocks.evictExtractedCache.mockImplementation(async (sha256: string) => {
      entries = entries.filter((item) => item.sha256 !== sha256);
    });
    renderScreen();

    await screen.findByText("aaaaaaaaaaaa…");
    expect(mocks.extractedCache).toHaveBeenCalledTimes(1);

    const rowButtons = screen.getAllByRole("button", { name: "Remove entry" });
    fireEvent.click(rowButtons[0]);

    const dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Remove entry" }));

    await waitFor(() => expect(mocks.evictExtractedCache).toHaveBeenCalledWith(SHA_ONE));
    await waitFor(() => expect(screen.queryByTitle(SHA_ONE)).toBeNull());
    expect(screen.getByTitle(SHA_TWO)).toBeTruthy();
    // The optimistic row is reconciled by one refetch of the overview.
    await waitFor(() => expect(mocks.extractedCache.mock.calls.length).toBeGreaterThan(1));
    expect(mocks.push).toHaveBeenCalledWith({ title: "Cache entry removed", tone: "success" });
  });

  it("keeps the entry and shows the server error when eviction is refused", async () => {
    mocks.extractedCache.mockResolvedValue(overview());
    mocks.evictExtractedCache.mockRejectedValue(
      new ApiError(409, "cache_entry_in_use", "The entry is being served; retry once the transfer ends"),
    );
    renderScreen();

    await screen.findByText("aaaaaaaaaaaa…");
    fireEvent.click(screen.getAllByRole("button", { name: "Remove entry" })[0]);
    const dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Remove entry" }));

    await waitFor(() =>
      expect(mocks.push).toHaveBeenCalledWith({
        title: "The cache entry could not be removed",
        description: "The entry is being served; retry once the transfer ends",
        tone: "danger",
      }),
    );
    // The optimistic removal was rolled back.
    expect(screen.getByTitle(SHA_ONE)).toBeTruthy();
  });
});
