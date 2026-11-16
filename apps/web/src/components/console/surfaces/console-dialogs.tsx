import { AnimatePresence } from "motion/react";
import type { GameSummary } from "@/lib/types";
import type { CueId } from "@/lib/ui-sound-specs";
import { LaunchPlate } from "@/components/big-picture/launch-overlay";
import type { LaunchPlateStatus, PlateRect } from "@/components/big-picture/launch-plate";
import { LayoutPicker } from "@/components/big-picture/layout-switcher";
import { ConfirmPanel } from "@/components/big-picture/overlays";
import type { BigPictureLayout } from "@/components/big-picture/layout";
import type { PowerAction } from "@/components/big-picture/power";
import type { PadFamily } from "@/components/big-picture/controller";

/**
 * The Console's three modal surfaces (#267): the launch handoff plate, the
 * power confirm and the quick layout switcher.
 *
 * They are the only things allowed to cover the shell, and each one is a top
 * input scope while it is open (the shell mounts `ConsoleOverlayInputScope`
 * with it). Everything else — menu, profile sheet, downloads, search,
 * collections — is a non-modal overlay or a route.
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
  pickerOpen: boolean;
  pickerIndex: number;
  onHighlightPicker: (index: number) => void;
  onSelectPicker: (layout: BigPictureLayout) => void;
  onClosePicker: () => void;
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
  pickerOpen,
  pickerIndex,
  onHighlightPicker,
  onSelectPicker,
  onClosePicker,
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
      <AnimatePresence initial={false}>
        {pickerOpen && (
          <LayoutPicker
            key="layout-picker"
            layout={layout}
            highlight={pickerIndex}
            reduceMotion={reduceMotion}
            onHighlight={onHighlightPicker}
            onSelect={onSelectPicker}
            onClose={onClosePicker}
          />
        )}
      </AnimatePresence>
    </>
  );
}
