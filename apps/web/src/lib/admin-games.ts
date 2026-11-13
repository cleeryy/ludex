/**
 * Admin Games table model (#209).
 *
 * The table is deep-linkable, so every filter lives in the query string. This
 * module owns the URL ⇄ query mapping, the sort labels and the status badges so
 * they can be unit-tested without rendering the screen.
 */

import type { AdminGameRow, AdminGameStatus, AdminGamesQuery } from "@/lib/types";
import { t } from "@/lib/i18n";

export const ADMIN_STATUS_VALUES = ["available", "partial", "missing", "all"] as const;
export type AdminStatusFilter = (typeof ADMIN_STATUS_VALUES)[number];

export const ADMIN_SORT_VALUES = [
  "title",
  "releaseDate",
  "addedAt",
  "updatedAt",
  "size",
  "versions",
  "missing",
  "installed",
] as const;
export type AdminSortValue = (typeof ADMIN_SORT_VALUES)[number];

export const ADMIN_PAGE_SIZE = 50;

export interface AdminGameFilters {
  q: string;
  status: AdminStatusFilter;
  provider: string;
  kind: "" | "game" | "rom";
  sort: AdminSortValue;
  order: "asc" | "desc";
  page: number;
}

export function defaultFilters(): AdminGameFilters {
  return { q: "", status: "available", provider: "", kind: "", sort: "title", order: "asc", page: 1 };
}

function isStatus(value: string | null): value is AdminStatusFilter {
  return value !== null && (ADMIN_STATUS_VALUES as readonly string[]).includes(value);
}

function isSort(value: string | null): value is AdminSortValue {
  return value !== null && (ADMIN_SORT_VALUES as readonly string[]).includes(value);
}

/** Reads the table state from the URL, falling back to safe defaults. */
export function readAdminGameFilters(params: URLSearchParams): AdminGameFilters {
  const status = params.get("status");
  const sort = params.get("sort");
  const order = params.get("order");
  const kind = params.get("kind");
  const page = Number(params.get("page") ?? "1");
  return {
    q: params.get("q") ?? "",
    status: isStatus(status) ? status : "available",
    provider: params.get("provider") ?? "",
    kind: kind === "game" || kind === "rom" ? kind : "",
    sort: isSort(sort) ? sort : "title",
    order: order === "desc" ? "desc" : "asc",
    page: Number.isFinite(page) && page >= 1 ? Math.floor(page) : 1,
  };
}

/** Writes the table state back to the URL, omitting defaults and empty values. */
export function writeAdminGameFilters(filters: AdminGameFilters): URLSearchParams {
  const params = new URLSearchParams();
  if (filters.q.trim()) params.set("q", filters.q.trim());
  if (filters.status !== "available") params.set("status", filters.status);
  if (filters.provider.trim()) params.set("provider", filters.provider.trim());
  if (filters.kind) params.set("kind", filters.kind);
  if (filters.sort !== "title") params.set("sort", filters.sort);
  if (filters.order !== "asc") params.set("order", filters.order);
  if (filters.page > 1) params.set("page", String(filters.page));
  return params;
}

/** Server query for the current table state. */
export function toAdminGamesQuery(filters: AdminGameFilters): AdminGamesQuery {
  return {
    q: filters.q.trim() || undefined,
    status: filters.status,
    provider: filters.provider.trim() || undefined,
    kind: filters.kind || undefined,
    sort: filters.sort,
    order: filters.order,
    page: filters.page,
    limit: ADMIN_PAGE_SIZE,
  };
}

/** A change to one filter resets the page: page 4 of a new search is empty. */
export function withFilter(
  filters: AdminGameFilters,
  patch: Partial<AdminGameFilters>,
): AdminGameFilters {
  return { ...filters, ...patch, page: patch.page ?? 1 };
}

export function statusLabel(status: AdminGameStatus | AdminStatusFilter): string {
  switch (status) {
    case "available":
      return t("Available");
    case "partially_missing":
    case "partial":
      return t("Partially missing");
    case "missing":
      return t("Missing");
    case "all":
      return t("All games");
  }
}

export function statusTone(status: AdminGameStatus): "ok" | "signal" | "danger" {
  switch (status) {
    case "available":
      return "ok";
    case "partially_missing":
      return "signal";
    case "missing":
      return "danger";
  }
}

/** Bucket used by the status tabs; the server reports the detail value. */
export function statusBucket(status: AdminGameStatus): Exclude<AdminStatusFilter, "all"> {
  return status === "partially_missing" ? "partial" : status;
}

export function sortLabelFor(value: AdminSortValue): string {
  switch (value) {
    case "title":
      return t("Title");
    case "releaseDate":
      return t("Release year");
    case "addedAt":
      return t("Added");
    case "updatedAt":
      return t("Updated");
    case "size":
      return t("Size");
    case "versions":
      return t("Versions");
    case "missing":
      return t("Missing versions");
    case "installed":
      return t("Installed reports");
  }
}

/**
 * The typed confirmation that gates a destructive bulk action: the operator
 * must type the exact number of games the preview reported.
 */
export function confirmationMatches(input: string, games: number): boolean {
  return input.trim() === String(games) && games > 0;
}

/** Row payload for a dry-run preview without a round trip. */
export function rowSelectionLabel(rows: AdminGameRow[], selected: Set<number>): string {
  const count = rows.filter((row) => selected.has(row.id)).length;
  return count === 1 ? t("1 selected") : t("{count} selected", { count });
}
