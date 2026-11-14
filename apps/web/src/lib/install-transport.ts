import type { GameVersion } from "@/lib/types";

/**
 * What the first byte of an install will actually do (#226).
 *
 * `stream` writes entries as they arrive and keeps no archive; `archive`
 * downloads the whole archive, verifies it, then extracts it; `unknown` means
 * the payload has no format metadata, so the dialog must not promise either.
 */
export type InstallTransport = "stream" | "archive" | "unknown";

/** Lower-case container name without the leading dot, or null when absent. */
export function normalizeArchiveFormat(format: string | null | undefined): string | null {
  const normalized = format?.trim().toLowerCase().replace(/^\./, "");
  return normalized ? normalized : null;
}

/**
 * Honest transport for a version, decided before the first byte (#226).
 *
 * Only ZIP archives can stream; 7z/RAR (and a portable installer) always take
 * the archive path. `streamSupported` is authoritative when the server sends
 * it, because the per-entry path also needs a manifest. A payload without any
 * format metadata is not a promise either way: the dialog says the transport is
 * checked when the download starts. Turning stream installs off makes the
 * archive path the only one that runs, whatever the format says.
 */
export function installTransport(
  version: Pick<GameVersion, "archiveFormat" | "streamSupported">,
  streamInstall = true,
): InstallTransport {
  if (!streamInstall) return "archive";
  if (version.streamSupported === true) return "stream";
  if (version.streamSupported === false) return "archive";
  const format = normalizeArchiveFormat(version.archiveFormat);
  if (!format) return "unknown";
  return format === "zip" ? "stream" : "archive";
}
