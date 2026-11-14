import { describe, expect, it } from "vitest";
import { installTransport, normalizeArchiveFormat } from "./install-transport";

describe("normalizeArchiveFormat", () => {
  it("lower-cases and strips the leading dot", () => {
    expect(normalizeArchiveFormat(".ZIP")).toBe("zip");
    expect(normalizeArchiveFormat(" 7z ")).toBe("7z");
  });

  it("returns null for missing or blank metadata", () => {
    expect(normalizeArchiveFormat(undefined)).toBeNull();
    expect(normalizeArchiveFormat(null)).toBeNull();
    expect(normalizeArchiveFormat("   ")).toBeNull();
  });
});

describe("installTransport", () => {
  it("streams when the server says the per-entry path is usable", () => {
    expect(installTransport({ streamSupported: true, archiveFormat: "zip" })).toBe("stream");
  });

  it("streams a ZIP when the payload predates streamSupported", () => {
    expect(installTransport({ archiveFormat: "zip" })).toBe("stream");
    expect(installTransport({ archiveFormat: ".ZIP" })).toBe("stream");
  });

  it("downloads 7z, RAR and portable installers", () => {
    expect(installTransport({ archiveFormat: "7z" })).toBe("archive");
    expect(installTransport({ archiveFormat: "rar" })).toBe("archive");
    expect(installTransport({ archiveFormat: "exe" })).toBe("archive");
  });

  it("trusts an explicit false even for a ZIP", () => {
    expect(installTransport({ streamSupported: false, archiveFormat: "zip" })).toBe("archive");
  });

  it("is unknown without any format metadata", () => {
    expect(installTransport({})).toBe("unknown");
    expect(installTransport({ archiveFormat: null, streamSupported: undefined })).toBe("unknown");
  });

  it("never promises a stream when stream installs are off", () => {
    expect(installTransport({ streamSupported: true, archiveFormat: "zip" }, false)).toBe("archive");
    expect(installTransport({}, false)).toBe("archive");
  });
});
