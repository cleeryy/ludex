import { describe, expect, it } from "vitest";
import {
  canStream,
  normalizeArchiveFormat,
  preflightCopy,
  resolveDownloadTransport,
} from "./install-transport";

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

const zip = { archiveFormat: "zip" };
const sevenZip = { archiveFormat: "7z" };
const rar = { archiveFormat: "rar" };
const noMetadata = { archiveFormat: null, streamSupported: undefined };

describe("resolveDownloadTransport", () => {
  it("is automatic by default and unresolved until the server answers", () => {
    expect(resolveDownloadTransport(zip)).toBe("automatic");
    expect(resolveDownloadTransport(sevenZip, "automatic", true)).toBe("automatic");
  });

  it("uses the files when the extracted cache is ready, for every container", () => {
    expect(resolveDownloadTransport(zip, "automatic", true, true)).toBe("files");
    expect(resolveDownloadTransport(sevenZip, "automatic", true, true)).toBe("files");
    expect(resolveDownloadTransport(rar, "automatic", false, true)).toBe("files");
  });

  it("falls back to the stream for a ZIP when the manifest is unavailable", () => {
    expect(resolveDownloadTransport(zip, "automatic", true, false)).toBe("stream");
    expect(resolveDownloadTransport({ archiveFormat: ".ZIP" }, "automatic", true, false)).toBe(
      "stream",
    );
    expect(
      resolveDownloadTransport({ streamSupported: true, archiveFormat: "zip" }, "automatic", true, false),
    ).toBe("stream");
  });

  it("falls back to the archive for 7z, RAR and portable installers", () => {
    expect(resolveDownloadTransport(sevenZip, "automatic", true, false)).toBe("archive");
    expect(resolveDownloadTransport(rar, "automatic", true, false)).toBe("archive");
    expect(
      resolveDownloadTransport({ archiveFormat: "exe" }, "automatic", true, false),
    ).toBe("archive");
  });

  it("trusts an explicit streamSupported false even for a ZIP", () => {
    expect(
      resolveDownloadTransport({ streamSupported: false, archiveFormat: "zip" }, "automatic", true, false),
    ).toBe("archive");
    expect(canStream({ streamSupported: false, archiveFormat: "zip" })).toBe(false);
  });

  it("never streams when the stream installs switch is off", () => {
    expect(resolveDownloadTransport(zip, "automatic", false, false)).toBe("archive");
    expect(resolveDownloadTransport(zip, "stream", false, false)).toBe("archive");
    expect(resolveDownloadTransport(zip, "automatic", false, true)).toBe("files");
    expect(resolveDownloadTransport(noMetadata, "automatic", false, false)).toBe("unknown");
  });

  it("always archives under the classic policy", () => {
    expect(resolveDownloadTransport(zip, "classic", true, true)).toBe("archive");
    expect(resolveDownloadTransport(sevenZip, "classic", true, false)).toBe("archive");
  });

  it("streams under the stream policy only when the server can", () => {
    expect(resolveDownloadTransport(zip, "stream", true, true)).toBe("stream");
    expect(resolveDownloadTransport(sevenZip, "stream", true, true)).toBe("archive");
    expect(resolveDownloadTransport(rar, "stream", true, false)).toBe("archive");
  });

  it("is unknown without any format metadata", () => {
    expect(resolveDownloadTransport(noMetadata, "automatic", true, false)).toBe("unknown");
    expect(resolveDownloadTransport(noMetadata, "stream", true, false)).toBe("unknown");
  });
});

const sameVolume = { stagingOnInstall: false, sameVolume: true };
const staged = { stagingOnInstall: true, sameVolume: false };
const crossVolume = { stagingOnInstall: false, sameVolume: false };

describe("preflightCopy", () => {
  it("never promises an archive when files will be used", () => {
    const copy = preflightCopy("files", crossVolume);
    expect(copy).toContain("files from the server");
    expect(copy).toContain("No archive lands on this device");
  });

  it("names the resolution order for an unresolved automatic install", () => {
    const copy = preflightCopy("automatic", sameVolume);
    expect(copy).toContain("extracted cache");
    expect(copy).toContain("streamed ZIP");
    expect(copy).toContain("otherwise the archive");
  });

  it("matches a streamed install", () => {
    expect(preflightCopy("stream", sameVolume)).toBe("Streaming entries, no archive kept");
  });

  it("matches the classic archive on the install drive, one volume or two", () => {
    expect(preflightCopy("archive", staged)).toContain("staged on the install drive");
    expect(preflightCopy("archive", sameVolume)).toContain("twice the archive size");
    expect(preflightCopy("archive", crossVolume)).toContain("stored on another volume");
  });

  it("stays honest when the payload has no format metadata", () => {
    expect(preflightCopy("unknown", staged)).toContain("staged on the install drive");
    expect(preflightCopy("unknown", sameVolume)).toBe("Checked when the download starts");
  });
});
