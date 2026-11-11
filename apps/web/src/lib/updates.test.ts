import { describe, expect, it } from "vitest";
import type { UpdateStatus } from "@/lib/types";
import {
  channelNeedsConfirmation,
  edgeBuildNumber,
  isPrerelease,
  isVersionSkipped,
  releaseNoteBlocks,
  releaseUrl,
  shouldNotify,
  shouldPrompt,
  shouldShowBadge,
  stripInlineMarkdown,
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
