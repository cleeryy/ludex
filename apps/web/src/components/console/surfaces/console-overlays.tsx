import { AnimatePresence } from "motion/react";
import type { RefObject } from "react";
import type { Collection, ControllerProfile, InstallJob } from "@/lib/types";
import type { HapticsStrength } from "@/lib/gamepad-core";
import { errorMessage } from "@/lib/http";
import type { BigPictureOverlay } from "@/components/big-picture/input";
import type { BigPictureLayout } from "@/components/big-picture/layout";
import type { PadFamily, ProfileOption } from "@/components/big-picture/controller";
import {
  CollectionsOverlay,
  CommandMenu,
  DownloadsOverlay,
} from "@/components/big-picture/overlays";
import { ProfileOverlay } from "@/components/big-picture/profile-overlay";
import { SearchOverlay } from "@/components/big-picture/search-overlay";
import type { SearchCell } from "@/components/big-picture/search";
import { useActiveJobs } from "@/state/downloads";

/**
 * The Console's non-modal surfaces (#267): command menu, download queue,
 * profile sheet, couch search and the collection index.
 *
 * None of them is a modal: the game underneath keeps its selected ring, the
 * queue keeps running, and the surface closes back onto the exact card. They
 * used to be two hundred lines inside the screen; they are the same overlays
 * with the shell's state passed in, so the screen can be read one layer at a
 * time.
 */
export interface ConsoleOverlaysProps {
  overlay: BigPictureOverlay | null;
  active: number;
  onHighlight: (index: number) => void;
  onChoose: (index: number) => void;
  onClose: () => void;
  layout: BigPictureLayout;
  connected: boolean;
  family: PadFamily;
  swapAB: boolean;
  reduceMotion: boolean;
  instant: boolean;
  downloads: number;
  titleFor: (job: InstallJob) => string;
  profileOptions: readonly ProfileOption[];
  selectedProfileId: string;
  profile: ControllerProfile;
  pad: { label: string; fingerprint: string; count: number } | null;
  haptics: boolean;
  strength: HapticsStrength;
  searchInputRef: RefObject<HTMLInputElement | null>;
  query: string;
  onQuery: (value: string) => void;
  rows: readonly SearchCell[][];
  searchActive: number;
  onSearchHighlight: (index: number) => void;
  collections: readonly Collection[];
  collectionsLoading: boolean;
  collectionsError: unknown;
  onCollectionsRetry: () => void;
}

/**
 * The queue surface owns its subscription (#267): a progress tick repaints
 * these rows, never the shell around them.
 */
function ConsoleDownloadsOverlay(props: Omit<Parameters<typeof DownloadsOverlay>[0], "jobs">) {
  const jobs = useActiveJobs();
  return <DownloadsOverlay {...props} jobs={jobs} />;
}

export function ConsoleOverlays({
  overlay,
  active,
  onHighlight,
  onChoose,
  onClose,
  layout,
  connected,
  family,
  swapAB,
  reduceMotion,
  instant,
  downloads,
  titleFor,
  profileOptions,
  selectedProfileId,
  profile,
  pad,
  haptics,
  strength,
  searchInputRef,
  query,
  onQuery,
  rows,
  searchActive,
  onSearchHighlight,
  collections,
  collectionsLoading,
  collectionsError,
  onCollectionsRetry,
}: ConsoleOverlaysProps) {
  const shell = {
    layout,
    connected,
    family,
    swapAB,
    reduceMotion,
    instant,
    onClose,
  };

  return (
    <AnimatePresence initial={false}>
      {overlay === "menu" && (
        <CommandMenu
          key="menu"
          {...shell}
          context="menu"
          active={active}
          onHighlight={onHighlight}
          onChoose={onChoose}
          downloads={downloads}
        />
      )}
      {overlay === "downloads" && (
        <ConsoleDownloadsOverlay
          key="downloads"
          {...shell}
          context="downloads"
          active={active}
          onHighlight={onHighlight}
          onChoose={onChoose}
          titleFor={titleFor}
        />
      )}
      {overlay === "profiles" && (
        <ProfileOverlay
          key="profiles"
          {...shell}
          context="profiles"
          active={active}
          onHighlight={onHighlight}
          onChoose={onChoose}
          options={[...profileOptions]}
          selectedId={selectedProfileId}
          profile={profile}
          pad={pad}
          haptics={haptics}
          strength={strength}
        />
      )}
      {overlay === "search" && (
        <SearchOverlay
          key="search"
          {...shell}
          context="search"
          inputRef={searchInputRef}
          query={query}
          onQuery={onQuery}
          rows={rows}
          active={searchActive}
          onHighlight={onSearchHighlight}
          onChoose={onChoose}
        />
      )}
      {overlay === "collections" && (
        <CollectionsOverlay
          key="collections"
          {...shell}
          context="collections"
          active={active}
          onHighlight={onHighlight}
          onChoose={onChoose}
          collections={[...collections]}
          loading={collectionsLoading}
          error={collectionsError ? errorMessage(collectionsError) : null}
          onRetry={onCollectionsRetry}
        />
      )}
    </AnimatePresence>
  );
}
