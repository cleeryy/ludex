// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ProtonDbBadge, steamAppIdFromProvider } from "./protondb-badge";

const protondbRating = vi.fn();

vi.mock("@/lib/native", () => ({
  isNative: () => true,
  native: {
    protondbRating: (...args: unknown[]) => protondbRating(...args),
  },
}));

function renderBadge(props: Parameters<typeof ProtonDbBadge>[0]) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <ProtonDbBadge {...props} />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  protondbRating.mockReset();
});

afterEach(cleanup);

describe("steamAppIdFromProvider", () => {
  it("extracts Steam app ids from steam provider records and store URLs", () => {
    expect(steamAppIdFromProvider({ slug: "steam", id: "1145360", url: null })).toBe(1145360);
    expect(
      steamAppIdFromProvider({
        slug: "custom",
        id: "abc",
        url: "https://store.steampowered.com/app/367520/Hollow_Knight/",
      }),
    ).toBe(367520);
    expect(steamAppIdFromProvider({ slug: "igdb", id: "123", url: "https://www.igdb.com/games/hades" })).toBeNull();
    expect(steamAppIdFromProvider(null)).toBeNull();
  });
});

describe("ProtonDbBadge", () => {
  it("passes resolved Steam appId and title to the desktop bridge and renders the tier badge", async () => {
    protondbRating.mockResolvedValue({
      appId: 1145360,
      status: "ok",
      tier: "platinum",
      bestTier: "platinum",
      trendingTier: "platinum",
      confidence: "strong",
      total: 761,
      score: 0.91,
      url: "https://www.protondb.com/app/1145360",
      fetchedAt: "2026-11-25T00:00:00Z",
      message: null,
    });

    renderBadge({
      gameId: 7,
      provider: { slug: "steam", id: "1145360", url: "https://store.steampowered.com/app/1145360" },
      title: "Hades",
      kind: "game",
    });

    const link = await screen.findByRole("link", { name: /ProtonDB Platinum/i });
    expect(link.getAttribute("href")).toBe("https://www.protondb.com/app/1145360");
    expect(protondbRating).toHaveBeenCalledWith(7, 1145360, false, "Hades");
  });

  it("renders native Linux tier badges and skips ROM entries", async () => {
    protondbRating.mockResolvedValue({
      appId: 367520,
      status: "ok",
      tier: "native",
      bestTier: "native",
      trendingTier: "native",
      confidence: "strong",
      total: 410,
      score: 0.96,
      url: "https://www.protondb.com/app/367520",
      fetchedAt: "2026-11-25T00:00:00Z",
      message: null,
    });

    renderBadge({ gameId: 8, provider: { slug: "igdb", id: "99", url: null }, title: "Hollow Knight", kind: "game" });
    expect(await screen.findByRole("link", { name: /ProtonDB Native/i })).toBeTruthy();
    expect(protondbRating).toHaveBeenCalledWith(8, null, false, "Hollow Knight");

    cleanup();
    protondbRating.mockReset();
    const { container } = renderBadge({ gameId: 9, title: "Super Metroid", kind: "rom" });
    expect(container.firstChild).toBeNull();
    expect(protondbRating).not.toHaveBeenCalled();
  });
});
