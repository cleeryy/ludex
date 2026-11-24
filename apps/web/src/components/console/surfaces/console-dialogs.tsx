import { AnimatePresence } from "motion/react";
import type { GameDetail, GameSummary, GameVersion } from "@/lib/types";
import type { CueId } from "@/lib/ui-sound-specs";
import { LaunchPlate } from "@/components/big-picture/launch-overlay";
import type { LaunchPlateStatus, PlateRect } from "@/components/big-picture/launch-plate";
import { ConfirmPanel } from "@/components/big-picture/overlays";
import { ConsoleInstallModal } from "@/components/console/surfaces/console-install-modal";
import type { BigPictureLayout } from "@/components/big-picture/layout";
import type { PowerAction } from "@/components/big-picture/power";
import type { PadFamily } from "@/components/big-picture/controller";

/**
 * The Console's modals (#267, #268, #284): the launch handoff plate, the
 * console-native install confirmation, and the power confirmation. Zero desktop
 * `InstallDialog` portals or `aria-modal` escapes ever mount inside Console.
 */
export interface ConsoleDialogsProps {
  layout: BigPictureLayout;
  connected: boolean;
  family: PadFamily;
  swapAB: boolean;
  reduceMotion: boolean;
  instant: boolean;
  launchPlate: {
    game: GameSummary;
    rect: PlateRect | null;
    status: LaunchPlateStatus;
    detail: string | null;
  } | null;
  onDismissPlate: () => void;
  pendingPower: { action: PowerAction; index: number } | null;
  onConfirmPower: (index: number) => void;
  onHighlightPower: (index: number) => void;
  onClosePower: () => void;
  installModal?: { game: GameDetail; version: GameVersion } | null;
  onInstalled?: () => void;
  onCloseInstall?: () => void;
  emit: (cue: CueId | null) => void;
}

export function ConsoleDialogs({
  layout,
  connected,
  family,
  swapAB,
  reduceMotion,
  instant,
  launchPlate,
  onDismissPlate,
  pendingPower,
  onConfirmPower,
  onHighlightPower,
  onClosePower,
  installModal = null,
  onInstalled,
  onCloseInstall,
  emit,
}: ConsoleDialogsProps) {
  return (
    <>
      <AnimatePresence initial={false}>
        {launchPlate && (
          <LaunchPlate
            key={`launch-${launchPlate.game.id}`}
            gameId={launchPlate.game.id}
            title={launchPlate.game.title}
            coverUrl={launchPlate.game.coverUrl}
            rect={launchPlate.rect}
            status={launchPlate.status}
            detail={launchPlate.detail}
            reduceMotion={reduceMotion}
            instant={instant}
            onDismiss={() => {
              emit("click");
              onDismissPlate();
            }}
          />
        )}
      </AnimatePresence>
      <AnimatePresence initial={false}>
        {installModal && onInstalled && onCloseInstall && (
          <ConsoleInstallModal
            key={`install-${installModal.game.id}-${installModal.version.id}`}
            game={installModal.game}
            version={installModal.version}
            layout={layout}
            connected={connected}
            family={family}
            swapAB={swapAB}
            reduceMotion={reduceMotion}
            instant={instant}
            onInstalled={onInstalled}
            onClose={onCloseInstall}
          />
        )}
      </AnimatePresence>
      <AnimatePresence initial={false}>
        {pendingPower && (
          <ConfirmPanel
            key="power-confirm"
            title={pendingPower.action.label}
            detail={pendingPower.action.detail}
            confirmLabel={pendingPower.action.label}
            tone={pendingPower.action.tone === "danger" ? "danger" : undefined}
            active={pendingPower.index}
            onHighlight={onHighlightPower}
            onChoose={onConfirmPower}
            context="exit"
            layout={layout}
            connected={connected}
            family={family}
            swapAB={swapAB}
            reduceMotion={reduceMotion}
            instant={instant}
            onClose={onClosePower}
          />
        )}
      </AnimatePresence>
    </>
  );
}
