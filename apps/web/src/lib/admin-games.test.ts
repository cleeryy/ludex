import { describe, expect, it } from "vitest";
import type { AdminGameRow } from "./types";
import {
  ADMIN_PAGE_SIZE,
  confirmationMatches,
  defaultFilters,
  readAdminGameFilters,
  rowSelectionLabel,
  statusBucket,
  statusLabel,
  statusTone,
  toAdminGamesQuery,
  withFilter,
  writeAdminGameFilters,
} from "./admin-games";

function row(id: number, patch: Partial<AdminGameRow> = {}): AdminGameRow {
  return {
    id,
    title: `Game ${id}`,
    releaseDate: null,
    kind: "game",
    platform: null,
    providerSlug: null,
    coverUrl: null,
    createdAt: "2026-01-01T00:00:00Z",
    updatedAt: "2026-01-01T00:00:00Z",
    defaultVersionId: null,
    availableVersionCount: 1,
    missingVersionCount: 0,
    status: "available",
    totalSize: 10,
    installedReports: 0,
    lastInstalledAt: null,
    ...patch,
  };
}

describe("admin game filters", () => {
  it("defaults to the available view on page 1", () => {
    const filters = readAdminGameFilters(new URLSearchParams());
    expect(filters).toEqual({
      q: "",
      status: "available",
      provider: "",
      kind: "",
      sort: "title",
      order: "asc",
      page: 1,
    });
    expect(toAdminGamesQuery(filters)).toMatchObject({
      status: "available",
      sort: "title",
      order: "asc",
      page: 1,
      limit: ADMIN_PAGE_SIZE,
    });
  });

  it("ignores unknown values instead of sending them to the server", () => {
    const filters = readAdminGameFilters(
      new URLSearchParams("status=broken&sort=nope&order=sideways&kind=film&page=-3"),
    );
    expect(filters.status).toBe("available");
    expect(filters.sort).toBe("title");
    expect(filters.order).toBe("asc");
    expect(filters.kind).toBe("");
    expect(filters.page).toBe(1);
  });

  it("round-trips a deep link and omits defaults", () => {
    const params = new URLSearchParams("q=hades&status=missing&sort=updatedAt&order=desc&page=3&kind=rom");
    const filters = readAdminGameFilters(params);
    expect(filters).toMatchObject({
      q: "hades",
      status: "missing",
      sort: "updatedAt",
      order: "desc",
      kind: "rom",
      page: 3,
    });
    expect(writeAdminGameFilters(filters).toString()).toBe(
      "q=hades&status=missing&kind=rom&sort=updatedAt&order=desc&page=3",
    );
    expect(writeAdminGameFilters(defaultFilters()).toString()).toBe("");
  });

  it("resets the page whenever a filter changes", () => {
    const filters = { ...defaultFilters(), page: 4 };
    expect(withFilter(filters, { q: "celeste" }).page).toBe(1);
    expect(withFilter(filters, { status: "all" }).page).toBe(1);
    expect(withFilter(filters, { page: 5 }).page).toBe(5);
  });

  it("maps server statuses to tab buckets", () => {
    expect(statusBucket("partially_missing")).toBe("partial");
    expect(statusBucket("missing")).toBe("missing");
    expect(statusBucket("available")).toBe("available");
    expect(statusTone("available")).toBe("ok");
    expect(statusTone("partially_missing")).toBe("signal");
    expect(statusTone("missing")).toBe("danger");
    expect(statusLabel("partial")).toBe(statusLabel("partially_missing"));
  });
});

describe("bulk confirmation", () => {
  it("requires the exact number of selected games", () => {
    expect(confirmationMatches("3", 3)).toBe(true);
    expect(confirmationMatches(" 3 ", 3)).toBe(true);
    expect(confirmationMatches("4", 3)).toBe(false);
    expect(confirmationMatches("", 3)).toBe(false);
    // A preview with zero games can never be confirmed: nothing to do.
    expect(confirmationMatches("0", 0)).toBe(false);
  });

  it("counts only the selected rows on the current page", () => {
    const rows = [row(1), row(2), row(3)];
    expect(rowSelectionLabel(rows, new Set([1, 2]))).toContain("2");
    expect(rowSelectionLabel(rows, new Set([99]))).toContain("0");
  });
});
