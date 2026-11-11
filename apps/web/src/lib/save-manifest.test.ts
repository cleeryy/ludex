import { describe, expect, it } from "vitest";
import type { ManifestStatus } from "./types";
import {
  formatManifestVersion,
  manifestActions,
  manifestState,
  manifestTone,
} from "./save-manifest";

function status(patch: Partial<ManifestStatus> = {}): ManifestStatus {
  return {
    present: true,
    path: "/home/mara/.local/share/ludex/manifest/ludusavi-manifest.yaml",
    version: "sha256:7c1f5d2a9b04",
    updatedAt: "2026-11-10T08:15:00Z",
    checkedAt: "2026-11-11T08:15:00Z",
    source: "https://raw.githubusercontent.com/mtkennerly/ludusavi-manifest/data/data/manifest.yaml",
    sizeBytes: 14_512_640,
    autoUpdate: true,
    stale: false,
    error: null,
    message: "The save database is ready.",
    ...patch,
  };
}

describe("save database panel state (#153)", () => {
  it("reports a missing copy as missing and offers only a download", () => {
    const missing = status({ present: false, path: null, version: null, stale: true });
    expect(manifestState(missing)).toBe("missing");
    expect(manifestActions(missing)).toEqual({ canUpdate: true, canRemove: false });
    expect(manifestTone("missing")).toBe("outline");
    expect(manifestState(null)).toBe("missing");
  });

  it("distinguishes ready, stale and a failed refresh", () => {
    expect(manifestState(status())).toBe("ready");
    expect(manifestState(status({ stale: true }))).toBe("stale");
    expect(manifestState(status({ error: "HTTP 404" }))).toBe("error");
    expect(manifestTone("ready")).toBe("ok");
    expect(manifestTone("stale")).toBe("signal");
  });

  it("keeps Remove available only when a copy exists", () => {
    expect(manifestActions(status())).toEqual({ canUpdate: true, canRemove: true });
    expect(manifestActions(status({ present: false, path: null })).canRemove).toBe(false);
  });

  it("shortens long versions without losing the algorithm prefix", () => {
    expect(formatManifestVersion("sha256:7c1f5d2a9b04")).toBe("sha256:7c1f5d2a9b04");
    expect(formatManifestVersion("sha256:0123456789abcdef0123456789abcdef")).toBe(
      "sha256:0123456789abcdef…",
    );
    expect(formatManifestVersion('W/"abc123"')).toBe('W/"abc123"');
    expect(formatManifestVersion(null)).toBe("");
    expect(formatManifestVersion("   ")).toBe("");
  });
});
