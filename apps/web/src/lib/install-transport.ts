import type { DownloadPolicy, GameVersion } from "@/lib/types";
import { t } from "@/lib/i18n";

/**
 * What the install dialog can promise before the first byte (#235).
 *
 * `files` is the launcher path: the server's extracted tree, verified file by
 * file, no archive on this device. `stream` writes ZIP entries as they arrive
 * and keeps no archive. `archive` downloads the whole archive, verifies it,
 * then extracts it. `automatic` means the desktop has not probed the server
 * yet: files when available, a stream for a ZIP, the archive otherwise.
 * `unknown` means the payload has no format metadata at all.
 */
export type DownloadTransport = "files" | "stream" | "archive" | "automatic" | "unknown";

/** Lower-case container name without the leading dot, or null when absent. */
export function normalizeArchiveFormat(format: string | null | undefined): string | null {
  const normalized = format?.trim().toLowerCase().replace(/^\./, "");
  return normalized ? normalized : null;
}

/**
 * True when the server can serve this version as a per-entry stream (#226).
 *
 * Only ZIP archives can stream; 7z/RAR (and a portable installer) always take
 * the archive path. `streamSupported` is authoritative when the server sends
 * it, because the per-entry path also needs a manifest. Turning stream installs
 * off makes the archive path the only one that runs, whatever the format says.
 */
export function canStream(
  version: Pick<GameVersion, "archiveFormat" | "streamSupported">,
  streamInstall = true,
): boolean {
  if (!streamInstall) return false;
  if (version.streamSupported === true) return true;
  if (version.streamSupported === false) return false;
  return normalizeArchiveFormat(version.archiveFormat) === "zip";
}

/** The payload says nothing about its format: the dialog must not guess. */
function hasFormatMetadata(
  version: Pick<GameVersion, "archiveFormat" | "streamSupported">,
): boolean {
  return Boolean(normalizeArchiveFormat(version.archiveFormat)) || version.streamSupported !== undefined;
}

/**
 * Resolve `downloadPolicy` for one version (#235), in the documented order:
 * file manifest → ZIP stream → classic archive.
 *
 * `fileManifest` is the server probe: `true` means a ready extracted cache,
 * `false` a 409 `extracted_cache_unavailable` (or a disabled cache). When it is
 * omitted the dialog has not asked yet and reports `automatic`.
 */
export function resolveDownloadTransport(
  version: Pick<GameVersion, "archiveFormat" | "streamSupported">,
  policy: DownloadPolicy = "automatic",
  streamInstall = true,
  fileManifest?: boolean,
): DownloadTransport {
  if (policy === "classic") return "archive";
  if (policy === "stream") {
    if (canStream(version, streamInstall)) return "stream";
    return hasFormatMetadata(version) ? "archive" : "unknown";
  }
  if (fileManifest === true) return "files";
  if (fileManifest === false) {
    if (canStream(version, streamInstall)) return "stream";
    return hasFormatMetadata(version) ? "archive" : "unknown";
  }
  return "automatic";
}

/** The honest preflight line the install dialog shows (#226/#235). */
export function preflightCopy(
  transport: DownloadTransport,
  context: { stagingOnInstall: boolean; sameVolume: boolean },
): string {
  switch (transport) {
    case "files":
      return t("Verified files from the server, one by one. No archive lands on this device.");
    case "automatic":
      return t("Automatic: files from the server when its extracted cache is ready, otherwise a streamed ZIP, otherwise the archive.");
    case "stream":
      return t("Streaming entries, no archive kept");
    case "archive":
      if (context.stagingOnInstall) {
        return t("Download the archive, then extract. It is staged on the install drive and deleted afterwards unless you keep archives.");
      }
      return context.sameVolume
        ? t("Download the archive, then extract. Peak usage is roughly twice the archive size.")
        : t("Download the archive, then extract. The archive is stored on another volume, then unpacked into the install location.");
    default:
      return context.stagingOnInstall
        ? t("Checked when the download starts. If it downloads an archive, it is staged on the install drive.")
        : t("Checked when the download starts");
  }
}
