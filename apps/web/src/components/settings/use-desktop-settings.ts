import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { errorMessage } from "@/lib/http";
import { installBridge, installsAvailable } from "@/lib/install";
import { isNative, native } from "@/lib/native";
import { setTitleBarPreference } from "@/lib/window-chrome";
import type { LocalSettings, LauncherId } from "@/lib/types";
import { useToast } from "@/state/toasts";
import { t } from "@/lib/i18n";

export interface DesktopSettingsController {
  /** Installs (and therefore the desktop settings) exist on this surface. */
  available: boolean;
  /** The saved snapshot, as last read from the bridge. */
  settings: LocalSettings | null;
  /** The editable copy; `null` until the first read lands. */
  draft: LocalSettings | null;
  dirty: boolean;
  saving: boolean;
  pending: boolean;
  error: unknown;
  update: (patch: Partial<LocalSettings>) => void;
  save: () => Promise<void>;
  pick: (key: "steamRoot", title: string) => Promise<void>;
  /** The title bar belongs to the window: applied and saved on its own (#111). */
  applyNativeTitleBar: (next: boolean) => Promise<void>;
  /** Launcher sources save themselves (#155). */
  applyLauncherSources: (sources: LauncherId[]) => Promise<void>;
  openInstallFolder: () => void;
}

/**
 * Desktop settings, shared by every category that edits the same draft.
 *
 * Settings are one file, so the categories cannot each own a save. The draft
 * and the save live here; a category renders the save bar, so the button sits
 * with the controls it commits (#174).
 */
export function useDesktopSettings(): DesktopSettingsController {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const available = installsAvailable();

  const [draft, setDraft] = useState<LocalSettings | null>(null);
  const [saving, setSaving] = useState(false);

  const query = useQuery({
    queryKey: ["desktop-settings"],
    queryFn: () => installBridge.settings(),
    enabled: available,
  });
  const settings = query.data ?? null;

  useEffect(() => {
    if (settings) setDraft(settings);
  }, [settings]);

  const dirty = Boolean(draft && settings && JSON.stringify(draft) !== JSON.stringify(settings));

  const update = (patch: Partial<LocalSettings>) =>
    setDraft((current) => (current ? { ...current, ...patch } : current));

  /**
   * The title bar toggle lives in Appearance but belongs to the desktop
   * settings: it is applied and persisted on its own, so it never waits for a
   * save elsewhere (#111).
   */
  const applyNativeTitleBar = async (next: boolean) => {
    update({ nativeTitleBar: next });
    // The window chrome (AppShell and the bare screens) reads this mirror.
    setTitleBarPreference(next);
    try {
      await native.setNativeTitleBar(next);
      // Refresh the saved snapshot so "Unsaved changes" stays honest. Skipped
      // while the form holds other edits: the refetch resets the draft.
      if (!dirty) await queryClient.invalidateQueries({ queryKey: ["desktop-settings"] });
    } catch (cause) {
      update({ nativeTitleBar: !next });
      setTitleBarPreference(!next);
      push({ title: t("Could not change the title bar"), description: errorMessage(cause), tone: "danger" });
    }
  };

  const pick = async (key: "steamRoot", title: string) => {
    const path = await native.pickFolder(title);
    if (path) update({ [key]: path } as Partial<LocalSettings>);
    else if (!isNative()) {
      push({
        title: t("Folder picker needs the desktop app"),
        description: t("You can type a path here in the browser."),
      });
    }
  };

  /**
   * Launcher sources persist on their own (#155): the checkbox is the whole
   * interaction in Settings → Integrations, so it must not wait for the shared
   * save. The draft updates immediately (so a scan right after the click
   * already uses the new set) and the cached snapshot is patched with the
   * saved value, which keeps the "Unsaved changes" hint and the rest of the
   * form untouched.
   */
  const applyLauncherSources = async (sources: LauncherId[]) => {
    update({ launcherSources: sources });
    try {
      const saved = await installBridge.saveLauncherSources(sources);
      if (saved) {
        queryClient.setQueryData<LocalSettings | null>(["desktop-settings"], (current) =>
          current ? { ...current, launcherSources: saved.launcherSources } : current,
        );
      }
    } catch (cause) {
      update({ launcherSources: settings?.launcherSources ?? [] });
      push({
        title: t("Could not save the launcher sources"),
        description: errorMessage(cause),
        tone: "danger",
      });
    }
  };

  const save = async () => {
    if (!draft) return;
    setSaving(true);
    try {
      await installBridge.saveSettings(draft);
      await queryClient.invalidateQueries({ queryKey: ["desktop-settings"] });
      await queryClient.invalidateQueries({ queryKey: ["desktop-locations"] });
      await queryClient.invalidateQueries({ queryKey: ["desktop-storage-suggestions"] });
      await queryClient.invalidateQueries({ queryKey: ["title-bar-info"] });
      await queryClient.invalidateQueries({ queryKey: ["compat-scan"] });
      await queryClient.invalidateQueries({ queryKey: ["cloud-save-status"] });
      push({ title: t("Desktop settings saved"), tone: "success" });
    } catch (cause) {
      push({ title: t("Could not save the settings"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setSaving(false);
    }
  };

  const openInstallFolder = () => {
    if (!draft?.installDir) return;
    void installBridge
      .openPath(draft.installDir)
      .catch(() => push({ title: t("Folder paths live on the desktop") }));
  };

  return {
    available,
    settings,
    draft,
    dirty,
    saving,
    pending: query.isPending,
    error: query.isError ? query.error : null,
    update,
    save,
    pick,
    applyNativeTitleBar,
    applyLauncherSources,
    openInstallFolder,
  };
}
