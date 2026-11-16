import { AnimatePresence } from "motion/react";
import type { GameSummary } from "@/lib/types";
import type { CueId } from "@/lib/ui-sound-specs";
import { LaunchPlate } from "@/components/big-picture/launch-overlay";
import type { LaunchPlateStatus, PlateRect } from "@/components/big-picture/launch-plate";
import { ConfirmPanel } from "@/components/big-picture/overlays";
import type { BigPictureLayout } from "@/components/big-picture/layout";
import type { PowerAction } from "@/components/big-picture/power";
import type { PadFamily } from "@/components/big-picture/controller";

/**
 * The Console's two modals (#267, #268): the launch handoff plate and the power
 * confirmation. Nothing else may cover the shell — the command menu, the
 * profile sheet, downloads, search and collections are non-modal overlays or
 * routes. The quick layout switcher is gone: the composition toggle lives in
 * Settings, where a preference belongs.
 *
 * Each modal is a top input scope while it is open (the shell mounts
 * `ConsoleOverlayInputScope` with it).
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
