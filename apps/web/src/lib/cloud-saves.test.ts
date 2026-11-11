import { describe, expect, it } from "vitest";
import {
  DETECTION_MAX_AGE_MS,
  retryDelayMs,
  shouldAutoDetect,
  shouldToastFailure,
} from "@/lib/cloud-saves";
import { hasMoreMedia, mediaWindow, MEDIA_PAGE_SIZE } from "@/lib/media";

describe("cloud save status helpers (#159)", () => {
  const now = Date.parse("2026-11-11T12:00:00Z");

  it("detects when a game has no paths and was never detected", () => {
    expect(shouldAutoDetect({ paths: [], lastDetectionAt: null }, now)).toBe(true);
  });

  it("does not re-detect while a saved path exists", () => {
    expect(
      shouldAutoDetect({ paths: [{ path: "/x" }], lastDetectionAt: null }, now),
    ).toBe(false);
  });

  it("honours the detection freshness window", () => {
    const fresh = new Date(now - DETECTION_MAX_AGE_MS / 2).toISOString();
    const stale = new Date(now - DETECTION_MAX_AGE_MS - 1).toISOString();
    expect(shouldAutoDetect({ paths: [], lastDetectionAt: fresh }, now)).toBe(false);
    expect(shouldAutoDetect({ paths: [], lastDetectionAt: stale }, now)).toBe(true);
  });

  it("re-detects rather than looping when the timestamp is unreadable", () => {
    expect(shouldAutoDetect({ paths: [], lastDetectionAt: "not a date" }, now)).toBe(true);
  });

  it("never detects without a status", () => {
    expect(shouldAutoDetect(null, now)).toBe(false);
    expect(shouldAutoDetect(undefined, now)).toBe(false);
  });

  it("backs off manual retries and caps the delay", () => {
    expect(retryDelayMs(0)).toBe(0);
    expect(retryDelayMs(1)).toBe(2_000);
    expect(retryDelayMs(2)).toBe(4_000);
    expect(retryDelayMs(3)).toBe(8_000);
    expect(retryDelayMs(10)).toBe(30_000);
    expect(retryDelayMs(Number.NaN)).toBe(0);
  });

  it("toasts a failure once per streak", () => {
    expect(shouldToastFailure(null, "boom")).toBe(true);
    expect(shouldToastFailure("boom", "boom")).toBe(false);
    expect(shouldToastFailure("boom", "other")).toBe(true);
  });
});

describe("media window helper (#159)", () => {
  it("caps the first page", () => {
    expect(mediaWindow(500, 0)).toBe(MEDIA_PAGE_SIZE);
    expect(mediaWindow(500, MEDIA_PAGE_SIZE)).toBe(MEDIA_PAGE_SIZE);
  });

  it("never exceeds the item count", () => {
    expect(mediaWindow(3, 100)).toBe(3);
    expect(mediaWindow(0, 8)).toBe(0);
  });

  it("grows one page at a time", () => {
    expect(mediaWindow(40, MEDIA_PAGE_SIZE + MEDIA_PAGE_SIZE)).toBe(MEDIA_PAGE_SIZE * 2);
    expect(mediaWindow(40, MEDIA_PAGE_SIZE + 1)).toBe(MEDIA_PAGE_SIZE + 1);
    expect(mediaWindow(9, 20)).toBe(9);
  });

  it("reports whether another page exists", () => {
    expect(hasMoreMedia(20, MEDIA_PAGE_SIZE)).toBe(true);
    expect(hasMoreMedia(20, 20)).toBe(false);
    expect(hasMoreMedia(0, 0)).toBe(false);
  });
});
