import { describe, expect, it } from "vitest";
import type { RemoteInstall } from "@/lib/types";
import {
  deviceLabel,
  isPhoneViewport,
  isRemoteActive,
  isStandaloneDisplay,
  remoteLooksOffline,
  remotePercent,
  remoteStatusKey,
  remoteStatusTone,
  remoteSummary,
  shouldOfferRemoteInstall,
  REMOTE_STALE_AFTER_MS,
} from "./mobile";

const base: RemoteInstall = {
  id: "ri_1",
  gameId: 4,
  gameTitle: "Hades",
  coverUrl: null,
  versionId: 9,
  version: "1.2",
  sizeBytes: 1024,
  device: "Pixel 8",
  status: "queued",
  progress: 0,
  message: null,
  requestedAt: "2026-11-12T10:00:00Z",
  updatedAt: "2026-11-12T10:00:00Z",
  claimedAt: null,
  completedAt: null,
};

describe("remote install helpers", () => {
  it("keeps only the three in-flight statuses active", () => {
    for (const status of ["queued", "claimed", "downloading"] as const) {
      expect(isRemoteActive(status), status).toBe(true);
    }
    for (const status of ["ready", "failed", "cancelled"] as const) {
      expect(isRemoteActive(status), status).toBe(false);
    }
  });

  it("maps every status to a message id and a badge tone", () => {
    const statuses = ["queued", "claimed", "downloading", "ready", "failed", "cancelled"] as const;
    for (const status of statuses) {
      expect(remoteStatusKey(status).length).toBeGreaterThan(0);
      expect(remoteStatusTone(status)).toMatch(/default|signal|ok|danger|outline/);
    }
    expect(new Set(statuses.map(remoteStatusKey)).size).toBe(statuses.length);
  });

  it("shows an indeterminate bar until a download reports progress", () => {
    expect(remotePercent(base)).toBeNull();
    expect(remotePercent({ status: "claimed", progress: 0 })).toBeNull();
    expect(remotePercent({ status: "downloading", progress: 42 })).toBe(42);
    expect(remotePercent({ status: "downloading", progress: 400 })).toBe(100);
    expect(remotePercent({ status: "ready", progress: 12 })).toBe(100);
  });

  it("flags a queued request as offline only after the threshold", () => {
    const now = Date.parse(base.updatedAt) + REMOTE_STALE_AFTER_MS + 1;
    expect(remoteLooksOffline(base, now)).toBe(true);
    expect(remoteLooksOffline(base, Date.parse(base.updatedAt) + 1_000)).toBe(false);
    expect(remoteLooksOffline({ ...base, status: "claimed" }, now)).toBe(true);
    expect(remoteLooksOffline({ ...base, status: "downloading" }, now)).toBe(false);
    expect(remoteLooksOffline({ ...base, updatedAt: "not-a-date" }, now)).toBe(false);
  });

  it("groups the queue for the header", () => {
    expect(
      remoteSummary([
        base,
        { ...base, id: "ri_2", status: "downloading" },
        { ...base, id: "ri_3", status: "ready" },
        { ...base, id: "ri_4", status: "failed" },
        { ...base, id: "ri_5", status: "cancelled" },
      ]),
    ).toEqual({ active: 2, ready: 1, failed: 1 });
  });

  it("labels the requesting device without trusting it", () => {
    expect(deviceLabel("Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X)")).toBe("iPhone");
    expect(deviceLabel("Mozilla/5.0 (Linux; Android 15; Pixel 8)")).toBe("Android phone");
    expect(deviceLabel("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)")).toBe("Web browser");
  });

  it("recognises phone layout and an installed PWA", () => {
    expect(isPhoneViewport(390)).toBe(true);
    expect(isPhoneViewport(1440)).toBe(false);
    expect(isStandaloneDisplay("standalone")).toBe(true);
    expect(isStandaloneDisplay("fullscreen")).toBe(true);
    expect(isStandaloneDisplay("browser")).toBe(false);
  });

  it("offers remote installs only in the browser, once signed in", () => {
    expect(shouldOfferRemoteInstall({ native: false, signedIn: true })).toBe(true);
    expect(shouldOfferRemoteInstall({ native: true, signedIn: true })).toBe(false);
    expect(shouldOfferRemoteInstall({ native: false, signedIn: false })).toBe(false);
  });
});
