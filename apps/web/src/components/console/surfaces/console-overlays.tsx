import { useCallback, useEffect, useMemo, useState, type RefObject } from "react";
import { AnimatePresence } from "motion/react";
import type { AppNotification, Collection, ControllerProfile, InstallJob } from "@/lib/types";
import type { HapticsStrength } from "@/lib/gamepad-core";
import { errorMessage } from "@/lib/http";
import type { CueId } from "@/lib/ui-sound-specs";
import { t, tn } from "@/lib/i18n";
import type { BigPictureOverlay } from "@/components/big-picture/input";
import { clampIndex, type BigPictureLayout } from "@/components/big-picture/layout";
import type { PadFamily, ProfileOption } from "@/components/big-picture/controller";
import {
  CollectionsOverlay,
  CommandMenu,
  DownloadsOverlay,
  downloadOverlayEntries,
  downloadRowActions,
} from "@/components/big-picture/overlays";
import { ProfileOverlay } from "@/components/big-picture/profile-overlay";
import { SearchOverlay } from "@/components/big-picture/search-overlay";
import type { SearchCell } from "@/components/big-picture/search";
import { useInputScope } from "@/components/console/input/input-provider";
import { setFocusMark } from "@/components/console/input/focus-store";
import { useConsoleNotice } from "@/components/console/surfaces/console-notice";
import { ConsoleNotificationsSheet } from "@/components/console/surfaces/console-notifications-sheet";
import {
  useActiveJobs,
  useDownloadsActions,
  useHistoryJobs,
  useQueueSummary,
} from "@/state/downloads";

/**
 * The Console's non-modal surfaces (#267): command menu, full downloads
 * manager, notifications sheet, profile sheet, couch search and collections.
 *
 * None of them is a modal: the game underneath keeps its selected ring, the
 * queue keeps running, and the surface closes back onto the exact card.
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
  emit: (cue: CueId | null) => void;
  online: boolean;
  notifications: readonly AppNotification[];
  unreadNotifications: number;
  notificationsLoading: boolean;
  notificationsError: unknown;
  onNotificationsRetry: () => void;
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
 * The Downloads surface owns both its store subscriptions and its input scope
 * (#267): a 1 Hz progress tick repaints this sheet only, never `ConsoleShell`,
 * and every action (Pause/Resume/Retry/Cancel/Open folder/Remove/Pause all/
 * Clear finished) stays 100% inside Console.
 */
function ConsoleDownloadsOverlay({
  titleFor,
  emit,
  onClose,
  ...shell
}: Omit<
  Parameters<typeof DownloadsOverlay>[0],
  "jobs" | "history" | "summary" | "selectedJob" | "active" | "onChoose" | "onHighlight"
> & {
  emit: (cue: CueId | null) => void;
}) {
  const jobs = useActiveJobs();
  const history = useHistoryJobs();
  const summary = useQueueSummary();
  const actions = useDownloadsActions();
  const { notify } = useConsoleNotice();

  const [listIndex, setListIndex] = useState(0);
  const [selectedJobId, setSelectedJobId] = useState<string | null>(null);
  const [actionIndex, setActionIndex] = useState(0);

  const entries = useMemo(() => downloadOverlayEntries(jobs, history), [jobs, history]);
  const selectedJob = useMemo(
    () =>
      selectedJobId
        ? jobs.find((entry) => entry.jobId === selectedJobId) ??
          history.find((entry) => entry.jobId === selectedJobId) ??
          null
        : null,
    [selectedJobId, jobs, history],
  );
  const rowActions = useMemo(
    () => (selectedJob ? downloadRowActions(selectedJob) : []),
    [selectedJob],
  );

  // If the selected job disappears (e.g. removed/cleared), drop back to the queue list.
  useEffect(() => {
    if (selectedJobId && !selectedJob) {
      setSelectedJobId(null);
      setActionIndex(0);
    }
  }, [selectedJobId, selectedJob]);

  const count = selectedJob ? rowActions.length : entries.length;
  const active = clampIndex(selectedJob ? actionIndex : listIndex, Math.max(1, count));

  useEffect(() => {
    const node = document.querySelector<HTMLElement>("[data-overlay-active='true']");
    if (node) setFocusMark(node);
  }, [active, selectedJobId, count]);

  const setActive = useCallback(
    (next: number) => {
      if (selectedJob) setActionIndex(next);
      else setListIndex(next);
    },
    [selectedJob],
  );

  const handleChoose = useCallback(
    (index: number) => {
      if (selectedJob) {
        const chosen = rowActions[index];
        if (!chosen) return;
        if (chosen.id === "back") {
          emit("click");
          setSelectedJobId(null);
          setActionIndex(0);
          return;
        }
        emit("confirm");
        const title = titleFor(selectedJob);
        setSelectedJobId(null);
        setActionIndex(0);
        switch (chosen.id) {
          case "pause":
            void actions.pause(selectedJob.jobId);
            return;
          case "resume":
            void actions.resume(selectedJob, { title });
            return;
          case "retry":
            void actions.retryAsDownload(selectedJob, { title });
            return;
          case "cancel":
            void actions.cancel(selectedJob.jobId);
            return;
          case "open-folder":
            if (selectedJob.installPath) void actions.openFolder(selectedJob.installPath);
            return;
          case "remove":
            void actions.remove(selectedJob.jobId);
            return;
        }
        return;
      }

      const entry = entries[index];
      if (!entry) return;
      if (entry.kind === "active" || entry.kind === "history") {
        emit("confirm");
        setSelectedJobId(entry.job.jobId);
        setActionIndex(0);
        return;
      }
      if (entry.kind === "pause-all") {
        emit("confirm");
        const running = jobs.filter((job) => job.phase !== "paused");
        void Promise.all(running.map((job) => actions.pause(job.jobId))).then(() => {
          notify({
            title: t("Downloads paused"),
            description: tn(running.length, "1 download paused", "{count} downloads paused"),
          });
        });
        return;
      }
      if (entry.kind === "clear-finished") {
        emit("confirm");
        void actions.clearHistory().then((removed) => {
          notify({
            title: t("History cleared"),
            description: tn(removed, "1 entry removed", "{count} entries removed"),
          });
        });
      }
    },
    [selectedJob, rowActions, entries, jobs, actions, titleFor, emit, notify],
  );

  const step = useCallback(
    (delta: -1 | 1) => {
      if (count <= 1) return;
      const next = clampIndex(active + delta, count);
      if (next !== active) {
        emit("move");
        setActive(next);
      }
    },
    [active, count, emit, setActive],
  );

  const handleBack = useCallback(() => {
    if (selectedJob) {
      emit("click");
      setSelectedJobId(null);
      setActionIndex(0);
      return;
    }
    onClose();
  }, [selectedJob, emit, onClose]);

  useInputScope({
    id: "console-downloads",
    onAction: (action) => {
      switch (action) {
        case "up":
        case "left":
          step(-1);
          return;
        case "down":
        case "right":
          step(1);
          return;
        case "confirm":
          handleChoose(active);
          return;
        case "back":
        case "menu":
          handleBack();
          return;
      }
    },
    onKey: (event) => {
      switch (event.key) {
        case "ArrowUp":
        case "ArrowLeft":
          event.preventDefault();
          step(-1);
          return true;
        case "ArrowDown":
        case "ArrowRight":
          event.preventDefault();
          step(1);
          return true;
        case "Enter":
          event.preventDefault();
          handleChoose(active);
          return true;
        case "Escape":
        case "Backspace":
          event.preventDefault();
          handleBack();
          return true;
        default:
          return false;
      }
    },
  });

  return (
    <DownloadsOverlay
      {...shell}
      onClose={handleBack}
      active={active}
      onHighlight={setActive}
      onChoose={handleChoose}
      jobs={jobs}
      history={history}
      summary={summary}
      selectedJob={selectedJob}
      titleFor={titleFor}
    />
  );
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
  emit,
  online,
  notifications,
  unreadNotifications,
  notificationsLoading,
  notificationsError,
  onNotificationsRetry,
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
          titleFor={titleFor}
          emit={emit}
        />
      )}
      {overlay === "notifications" && (
        <ConsoleNotificationsSheet
          key="notifications"
          {...shell}
          context="notifications"
          active={active}
          onHighlight={onHighlight}
          onChoose={onChoose}
          online={online}
          notifications={notifications}
          unreadCount={unreadNotifications}
          loading={notificationsLoading}
          error={notificationsError ? errorMessage(notificationsError) : null}
          onRetry={onNotificationsRetry}
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
