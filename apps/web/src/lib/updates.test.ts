import { afterEach, describe, expect, it, vi } from "vitest";
import type { UpdateStatus } from "@/lib/types";
import {
  channelNeedsConfirmation,
  clearUpdateNotice,
  edgeBuildNumber,
  isPrerelease,
  isVersionSkipped,
  markUpdateNoticeLater,
  markUpdateNoticeShown,
  readUpdateNotice,
  releaseNoteBlocks,
  releaseUrl,
  shouldNotify,
  shouldPrompt,
  shouldSendOsNotification,
  shouldShowBadge,
  shouldShowUpdateNotice,
  stripInlineMarkdown,
  UPDATE_NOTICE_STORAGE_KEY,
} from "@/lib/updates";

function status(overrides: Partial<UpdateStatus> = {}): UpdateStatus {
  return {
    configured: true,
    channel: "stable",
    currentVersion: "0.3.1",
    lastCheckedAt: null,
    endpoint: "https://github.com/cleeryy/ludex/releases/latest/download/latest.json",
    skippedVersion: null,
    notify: false,
    available: null,
    message: null,
    ...overrides,
  };
}

const update = {
  version: "0.3.2",
  currentVersion: "0.3.1",
  notes: null,
  publishedAt: null,
  releaseUrl: "https://github.com/cleeryy/ludex/releases/tag/v0.3.2",
  isPrerelease: false,
  build: null,
  sizeBytes: null,
};

describe("update channel and release links", () => {
  it("links stable versions to their tag and early to the rolling edge release", () => {
    expect(releaseUrl("stable", "0.3.2")).toBe("https://github.com/cleeryy/ludex/releases/tag/v0.3.2");
    expect(releaseUrl("stable", "v0.3.2")).toBe("https://github.com/cleeryy/ludex/releases/tag/v0.3.2");
    expect(releaseUrl("stable", "")).toBe("https://github.com/cleeryy/ludex/releases/latest");
    expect(releaseUrl("early", "0.3.2-edge.179")).toBe("https://github.com/cleeryy/ludex/releases/tag/edge");
  });

  it("recognises edge builds and their run number", () => {
    expect(isPrerelease("0.3.2-edge.179")).toBe(true);
    expect(isPrerelease("0.3.2")).toBe(false);
    expect(edgeBuildNumber("0.3.2-edge.179")).toBe(179);
    expect(edgeBuildNumber("0.3.2-edge.1")).toBe(1);
    expect(edgeBuildNumber("0.3.2")).toBeNull();
    expect(edgeBuildNumber("0.3.2-edge.next")).toBeNull();
    expect(edgeBuildNumber("0.3.2-edge.0")).toBeNull();
  });

  it("only warns when entering early access from another channel", () => {
    expect(channelNeedsConfirmation("stable", "early")).toBe(true);
    expect(channelNeedsConfirmation("early", "stable")).toBe(false);
    expect(channelNeedsConfirmation("stable", "stable")).toBe(false);
  });
});

describe("skip and notification decisions", () => {
  it("skips exactly one version", () => {
    expect(isVersionSkipped("0.3.2", "0.3.2")).toBe(true);
    expect(isVersionSkipped(" 0.3.2 ", "0.3.2 ")).toBe(true);
    expect(isVersionSkipped("0.3.3", "0.3.2")).toBe(false);
    expect(isVersionSkipped("0.3.2-edge.180", "0.3.2-edge.179")).toBe(false);
    expect(isVersionSkipped("0.3.2", "  ")).toBe(false);
    expect(isVersionSkipped(null, "0.3.2")).toBe(false);
  });

  it("notifies about an available version unless it was skipped", () => {
    expect(shouldNotify("0.3.2", null)).toBe(true);
    expect(shouldNotify("0.3.2", "0.3.1")).toBe(true);
    expect(shouldNotify("0.3.2", "0.3.2")).toBe(false);
    expect(shouldNotify(null, null)).toBe(false);
    expect(shouldNotify("  ", "0.3.2")).toBe(false);
  });

  it("shows the badge only when the backend says a prompt is due", () => {
    expect(shouldShowBadge(status({ available: update, notify: true }))).toBe(true);
    expect(shouldShowBadge(status({ available: update, notify: false }))).toBe(false);
    expect(shouldShowBadge(status({ available: null, notify: false }))).toBe(false);
    expect(shouldShowBadge(null)).toBe(false);
  });

  it("opens the launch prompt only when nothing silences it", () => {
    const live = status({ available: update, notify: true });
    const base = {
      status: live,
      skippedVersion: null,
      remindAfter: null,
      promptedVersion: null,
      now: 1_000,
    };

    expect(shouldPrompt(base)).toBe(true);
    // No update at all.
    expect(shouldPrompt({ ...base, status: status() })).toBe(false);
    // Skipped this exact version.
    expect(shouldPrompt({ ...base, skippedVersion: "0.3.2" })).toBe(false);
    // Already prompted during this run.
    expect(shouldPrompt({ ...base, promptedVersion: "0.3.2" })).toBe(false);
    // A different prompted version does not block a newer one.
    expect(shouldPrompt({ ...base, promptedVersion: "0.3.1" })).toBe(true);
    // "Remind me later" is still active...
    expect(shouldPrompt({ ...base, remindAfter: 2_000 })).toBe(false);
    // ...and expires on its own.
    expect(shouldPrompt({ ...base, remindAfter: 500 })).toBe(true);
    // The backend already decided no notification is due.
    expect(
      shouldPrompt({ ...base, status: status({ available: update, notify: false }) }),
    ).toBe(false);
  });
});

describe("update notices (#213)", () => {
  const live = status({ available: update, notify: true });

  it("shows one notice per version and stays quiet afterwards", () => {
    const base = { status: live, preference: "app-and-os" as const, remindAfter: null, now: 1_000 };
    expect(shouldShowUpdateNotice({ ...base, notice: null })).toBe(true);
    // A shown record dedupes the same version on every later check/relaunch.
    expect(
      shouldShowUpdateNotice({ ...base, notice: { version: "0.3.2", state: "shown", until: null } }),
    ).toBe(false);
    // A newer version is a new notice.
    expect(
      shouldShowUpdateNotice({
        ...base,
        status: status({ available: { ...update, version: "0.3.3" }, notify: true }),
        notice: { version: "0.3.2", state: "shown", until: null },
      }),
    ).toBe(true);
    // No update, nothing to announce.
    expect(shouldShowUpdateNotice({ ...base, status: status(), notice: null })).toBe(false);
  });

  it("never announces a skipped version, and marking a notice read is not a skip", () => {
    const base = {
      status: live,
      preference: "app-and-os" as const,
      notice: null,
      remindAfter: null,
      now: 1_000,
    };
    // The backend already knows the version was skipped (status.notify false).
    expect(
      shouldShowUpdateNotice({ ...base, status: status({ available: update, notify: false }) }),
    ).toBe(false);
    // Read/dismiss leaves the update available: the badge still shows.
    expect(shouldShowBadge(status({ available: update, notify: true }))).toBe(true);
  });

  it("respects the reminder window for Later and the global preference", () => {
    const base = { status: live, notice: null, remindAfter: null, now: 1_000 };
    // Later on the same version: silent until the window ends...
    expect(
      shouldShowUpdateNotice({
        ...base,
        preference: "app-and-os",
        notice: { version: "0.3.2", state: "later", until: 2_000 },
      }),
    ).toBe(false);
    // ...then it comes back (the user asked to be reminded).
    expect(
      shouldShowUpdateNotice({
        ...base,
        preference: "app-and-os",
        now: 2_001,
        notice: { version: "0.3.2", state: "later", until: 2_000 },
      }),
    ).toBe(true);
    // The provider's own remind-later window silences it too.
    expect(
      shouldShowUpdateNotice({ ...base, preference: "app-and-os", remindAfter: 2_000 }),
    ).toBe(false);
    expect(shouldShowUpdateNotice({ ...base, preference: "app-and-os", remindAfter: 500 })).toBe(true);
    // Preference matrix: off hides the notice, app/app-and-os show it.
    expect(shouldShowUpdateNotice({ ...base, preference: "off" })).toBe(false);
    expect(shouldShowUpdateNotice({ ...base, preference: "app" })).toBe(true);
    // The update center is the source of truth: no notice while on it.
    expect(
      shouldShowUpdateNotice({ ...base, preference: "app-and-os", onUpdateCenter: true }),
    ).toBe(false);
  });

  it("only asks the OS when the window is in the background", () => {
    const base = { preference: "app-and-os" as const, focused: false, minimized: false };
    // Background/minimised: the OS banner is the only signal.
    expect(shouldSendOsNotification(base)).toBe(true);
    expect(shouldSendOsNotification({ ...base, minimized: true })).toBe(true);
    // Focused window: the in-app notice is already on screen.
    expect(shouldSendOsNotification({ ...base, focused: true })).toBe(false);
    expect(shouldSendOsNotification({ ...base, focused: true, minimized: true })).toBe(true);
    // Preference gates the OS channel only.
    expect(shouldSendOsNotification({ ...base, preference: "app" })).toBe(false);
    expect(shouldSendOsNotification({ ...base, preference: "off" })).toBe(false);
    // A running game or the console shell never gets a popup.
    expect(shouldSendOsNotification({ ...base, gameRunning: true })).toBe(false);
    expect(shouldSendOsNotification({ ...base, consoleActive: true })).toBe(false);
  });

  describe("local notice record", () => {
    afterEach(() => {
      vi.unstubAllGlobals();
      clearUpdateNotice();
    });

    it("round-trips shown and later records", () => {
      const entries = new Map<string, string>();
      vi.stubGlobal("localStorage", {
        getItem: (key: string) => entries.get(key) ?? null,
        setItem: (key: string, value: string) => entries.set(key, value),
        removeItem: (key: string) => entries.delete(key),
      });
      expect(readUpdateNotice()).toBeNull();
      markUpdateNoticeShown("0.5.0");
      expect(readUpdateNotice()).toEqual({ version: "0.5.0", state: "shown", until: null });
      const until = markUpdateNoticeLater("0.5.0", 1_000, 5_000);
      expect(until).toBe(6_000);
      expect(readUpdateNotice()).toEqual({ version: "0.5.0", state: "later", until: 6_000 });
      expect(entries.has(UPDATE_NOTICE_STORAGE_KEY)).toBe(true);
      clearUpdateNotice();
      expect(readUpdateNotice()).toBeNull();
    });

    it("ignores a corrupt record", () => {
      const entries = new Map<string, string>([[UPDATE_NOTICE_STORAGE_KEY, "{not json"]]);
      vi.stubGlobal("localStorage", {
        getItem: (key: string) => entries.get(key) ?? null,
        setItem: () => undefined,
        removeItem: () => undefined,
      });
      expect(readUpdateNotice()).toBeNull();
    });
  });
});

describe("release notes", () => {
  it("rebuilds headings, bullets and paragraphs without interpreting HTML", () => {
    const blocks = releaseNoteBlocks(
      [
        "## Highlights",
        "",
        "A **new** library view and a faster install.",
        "",
        "- One fix",
        "- Two `fixes`",
        "",
        "<script>alert(1)</script>",
      ].join("\n"),
    );
    expect(blocks).toEqual([
      { kind: "heading", text: "Highlights" },
      { kind: "paragraph", text: "A new library view and a faster install." },
      { kind: "bullet", text: "One fix" },
      { kind: "bullet", text: "Two fixes" },
      { kind: "paragraph", text: "alert(1)" },
    ]);
  });

  it("keeps link labels and drops fences, rules and empty lines", () => {
    const blocks = releaseNoteBlocks(
      ["### Fixes", "```", "cargo test", "```", "---", "See [the PR](https://example.test/pr/1)."].join("\n"),
    );
    expect(blocks).toEqual([
      { kind: "heading", text: "Fixes" },
      { kind: "paragraph", text: "cargo test" },
      { kind: "paragraph", text: "See the PR." },
    ]);
  });

  it("returns no blocks for an empty automatic edge note", () => {
    expect(releaseNoteBlocks("")).toEqual([]);
    expect(releaseNoteBlocks("   \n\n")).toEqual([]);
    expect(releaseNoteBlocks(null)).toEqual([]);
    expect(releaseNoteBlocks(undefined)).toEqual([]);
  });

  it("strips emphasis, code and tags from a single line", () => {
    expect(stripInlineMarkdown("**Bold** and _italic_ and `code`")).toBe("Bold and italic and code");
    expect(stripInlineMarkdown("![shot](https://example.test/a.png) caption")).toBe("shot caption");
    expect(stripInlineMarkdown("<img src=x onerror=alert(1)>plain")).toBe("plain");
    expect(stripInlineMarkdown("Tom &amp; Jerry &quot;HD&quot;")).toBe('Tom & Jerry "HD"');
    expect(stripInlineMarkdown("  many   spaces  ")).toBe("many spaces");
  });
});
