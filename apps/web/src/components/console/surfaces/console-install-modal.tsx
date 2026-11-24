import { useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { cn } from "@/lib/cn";
import { errorMessage } from "@/lib/http";
import { formatBytes } from "@/lib/format";
import { t } from "@/lib/i18n";
import { peakEstimate } from "@/lib/install-phase";
import { isNative, native } from "@/lib/native";
import { qk } from "@/lib/query-keys";
import type { GameDetail, GameVersion, LocationStatus } from "@/lib/types";
import { useDownloadsActions } from "@/state/downloads";
import { OverlayRow, OverlayShell } from "@/components/big-picture/overlays";
import { Glyph } from "@/components/big-picture/glyphs";
import type { PadFamily } from "@/components/big-picture/controller";
import type { BigPictureLayout } from "@/components/big-picture/layout";
import { useInputScope } from "@/components/console/input/input-provider";
import { wrapIndex } from "@/components/big-picture/system-zones";

const FREE_SPACE_RESERVE = 256 * 1024 * 1024;

/**
 * Console-native Install confirmation modal (#268, #284).
 *
 * One of the two allowed Console modals (alongside Power confirmation). Zero
 * portals, zero desktop `InstallDialog`, 100% gamepad navigable: choose drive,
 * inspect free space vs. peak estimate, then Install or Cancel.
 */
export function ConsoleInstallModal({
  game,
  version,
  layout,
  connected,
  family,
  swapAB,
  reduceMotion,
  instant,
  onInstalled,
  onClose,
}: {
  game: GameDetail;
  version: GameVersion;
  layout: BigPictureLayout;
  connected: boolean;
  family: PadFamily;
  swapAB: boolean;
  reduceMotion: boolean;
  instant: boolean;
  onInstalled: () => void;
  onClose: () => void;
}) {
  const downloads = useDownloadsActions();
  const [driveIndex, setDriveIndex] = useState(0);
  const [rowIndex, setRowIndex] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const { data: locations } = useQuery<LocationStatus[]>({
    queryKey: qk.desktopLocations,
    queryFn: () => native.listLocations(),
    enabled: isNative(),
    staleTime: 20_000,
  });

  const installLocations = useMemo(
    () => (locations ?? []).filter((location) => location.kind === "install"),
    [locations],
  );
  const chosenDrive = installLocations[driveIndex] ?? null;
  const peak = peakEstimate(version.size);
  const needed = peak.peakBytes + FREE_SPACE_RESERVE;
  const enoughSpace = chosenDrive?.freeBytes == null || chosenDrive.freeBytes >= needed;
  const canInstall = !busy && enoughSpace && (!chosenDrive || chosenDrive.writable);

  const hasDriveRow = installLocations.length > 0;
  const installRow = hasDriveRow ? 1 : 0;
  const cancelRow = installRow + 1;
  const totalRows = cancelRow + 1;

  const startInstall = async () => {
    if (!canInstall) return;
    setBusy(true);
    setError(null);
    try {
      await downloads.startInstall({
        gameId: game.id,
        versionId: version.id,
        title: game.title,
        version: version.version,
        executable: game.launch.executable,
        locationId: chosenDrive?.id,
        expectedBytes: version.size,
      });
      onInstalled();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  };

  const activateRow = (index: number) => {
    if (hasDriveRow && index === 0) {
      setDriveIndex((current) => wrapIndex(current, 1, installLocations.length));
      return;
    }
    if (index === installRow) {
      void startInstall();
      return;
    }
    onClose();
  };

  useInputScope({
    id: "console-install-modal",
    padLayer: true,
    onAction: (action) => {
      if (action === "back") {
        onClose();
        return;
      }
      if (action === "up") {
        setRowIndex((current) => Math.max(0, current - 1));
        return;
      }
      if (action === "down") {
        setRowIndex((current) => Math.min(totalRows - 1, current + 1));
        return;
      }
      if ((action === "left" || action === "right") && hasDriveRow && rowIndex === 0) {
        const delta = action === "left" ? -1 : 1;
        setDriveIndex((current) => wrapIndex(current, delta, installLocations.length));
        return;
      }
      if (action === "confirm") {
        activateRow(rowIndex);
      }
    },
    onKey: (event) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
        return true;
      }
      if (event.key === "ArrowUp") {
        event.preventDefault();
        setRowIndex((current) => Math.max(0, current - 1));
        return true;
      }
      if (event.key === "ArrowDown") {
        event.preventDefault();
        setRowIndex((current) => Math.min(totalRows - 1, current + 1));
        return true;
      }
      if ((event.key === "ArrowLeft" || event.key === "ArrowRight") && hasDriveRow && rowIndex === 0) {
        event.preventDefault();
        setDriveIndex((current) => wrapIndex(current, event.key === "ArrowLeft" ? -1 : 1, installLocations.length));
        return true;
      }
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        activateRow(rowIndex);
        return true;
      }
      return false;
    },
  });

  return (
    <OverlayShell
      title={t("Install {title}", { title: game.title })}
      subtitle={t("Version {version}", { version: version.version })}
      context="exit"
      layout={layout}
      connected={connected}
      family={family}
      swapAB={swapAB}
      reduceMotion={reduceMotion}
      instant={instant}
      onClose={onClose}
    >
      <div className="mb-3 grid grid-cols-2 gap-3 rounded-[var(--bp-radius-object)] border border-line bg-bg-deep px-4 py-3">
        <div>
          <p className="text-bp-meta text-ink-3">{t("Download")}</p>
          <p className="num text-bp-body font-semibold text-ink">{formatBytes(version.size)}</p>
        </div>
        <div>
          <p className="text-bp-meta text-ink-3">{t("Peak space")}</p>
          <p className="num text-bp-body font-semibold text-info">{formatBytes(peak.peakBytes)}</p>
        </div>
      </div>

      {hasDriveRow && chosenDrive && (
        <OverlayRow
          active={rowIndex === 0}
          onHighlight={() => setRowIndex(0)}
          onChoose={() => activateRow(0)}
          icon={<Glyph name="library" className="size-[calc(var(--bp-u)*26)]" />}
          label={chosenDrive.label}
          detail={chosenDrive.path}
          trailing={
            <span className={cn("num", enoughSpace ? "text-ok" : "text-danger")}>
              {chosenDrive.freeBytes != null ? formatBytes(chosenDrive.freeBytes) : "—"}
            </span>
          }
        />
      )}

      {!enoughSpace && chosenDrive && (
        <p role="alert" className="px-3 py-2 text-bp-meta text-danger">
          {t("Not enough free space on {label}: {free} available, about {needed} needed.", {
            label: chosenDrive.label,
            free: formatBytes(chosenDrive.freeBytes ?? 0),
            needed: formatBytes(needed),
          })}
        </p>
      )}

      {error && (
        <p role="alert" className="px-3 py-2 text-bp-meta text-danger">
          {error}
        </p>
      )}

      <OverlayRow
        active={rowIndex === installRow}
        onHighlight={() => setRowIndex(installRow)}
        onChoose={() => activateRow(installRow)}
        icon={<Glyph name="install" className="size-[calc(var(--bp-u)*26)]" />}
        label={busy ? t("Starting") : t("Install")}
        detail={enoughSpace ? t("Adds the job to Console Downloads.") : undefined}
      />

      <OverlayRow
        active={rowIndex === cancelRow}
        onHighlight={() => setRowIndex(cancelRow)}
        onChoose={onClose}
        icon={<Glyph name="back" className="size-[calc(var(--bp-u)*26)]" />}
        label={t("Cancel")}
      />
    </OverlayShell>
  );
}
