import { useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { AlertTriangle, Lock, Play, RefreshCw, Wand2 } from "lucide-react";
import { errorMessage } from "@/lib/http";
import { isNative, native } from "@/lib/native";
import { formatBytes } from "@/lib/format";
import { displayCapabilityBlock } from "@/lib/display-profile";
import { applyableSteps, consentDownloads, totalDownloadBytes } from "@/lib/make-it-run";
import {
  AUDIO_DEVICE_ENV_KEY,
  readTier2Switch,
  setAdvancedTiersUnlocked,
  tier2SwitchesFor,
  tierOneState,
  useAdvancedTiersUnlocked,
  withTier2Switch,
  type Tier2SwitchId,
} from "@/lib/settings-tiers";
import type { GameDetail, InstalledGame, LaunchOptions } from "@/lib/types";
import { Badge, Button, Input, Switch } from "@/components/ui/controls";
import { Dialog } from "@/components/ui/dialog";
import { SectionHeader } from "@/components/page";
import { AudioOutputPanel } from "@/components/game/audio-output";
import { CompatRulesPanel } from "@/components/game/compat-rules";
import { LaunchOptionsPanel } from "@/components/game/launch-options";
import { LaunchPlanPanel } from "@/components/game/launch-plan";
import { MakeItRunPanel } from "@/components/game/make-it-run";
import { ShaderPrepPanel } from "@/components/game/shader-prep";
import { StartupCommandPanel } from "@/components/game/startup-command";
import {
  advancedReason,
  advancedWarning,
  settingsTiersDescription,
  settingsTiersTitle,
  tier2Explanation,
  tier2Intro,
  tier2Label,
  tierLabel,
  tierOneAction,
  tierOneDescription,
  tierOneTitle,
} from "@/components/game/settings-tiers-copy";
import { useToast } from "@/state/toasts";
import { t } from "@/lib/i18n";
import { qk } from "@/lib/query-keys";

/**
 * One settings surface, three tiers (#299).
 *
 * Tier 1 is a computed verdict and exactly one primary action. Tier 2 is six
 * curated switches that each write one existing profile field — see
 * `lib/settings-tiers.ts` for the mapping table and its tests. Tier 3 is the
 * old `LaunchOptionsPanel`, unchanged, behind a device-local unlock with an
 * explicit unsupported warning. Nothing was deleted: the interactive overlay
 * opt-out still lives in Settings → Profiles, and the raw launch options are
 * still the same component, just below the lock.
 */
export function SettingsTiersPanel({
  gameId,
  title,
  game,
  installed,
  isEditor,
}: {
  gameId: number;
  title: string;
  /** Tier 3 also owns the custom launch command, when the user may edit it. */
  game: GameDetail;
  installed?: InstalledGame;
  isEditor: boolean;
}) {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const unlocked = useAdvancedTiersUnlocked();

  const [savingSwitch, setSavingSwitch] = useState<Tier2SwitchId | null>(null);
  /** Null follows the profile; a string is the user's unsaved device draft. */
  const [audioDraft, setAudioDraft] = useState<string | null>(null);
  const [audioEditing, setAudioEditing] = useState(false);
  const [planOpen, setPlanOpen] = useState(false);
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [fixing, setFixing] = useState(false);

  const optionsQuery = useQuery({
    queryKey: qk.launchOptions(gameId),
    queryFn: () => native.getLaunchOptions(gameId),
    enabled: isNative(),
    retry: false,
  });
  const scanQuery = useQuery({
    queryKey: qk.compatScan,
    queryFn: () => native.compatScan(),
    enabled: isNative(),
    staleTime: 60_000,
    retry: false,
  });
  const diagnosisQuery = useQuery({
    queryKey: qk.makeItRun(gameId),
    queryFn: () => native.makeItRunDiagnose(gameId),
    enabled: isNative(),
    staleTime: 15_000,
    retry: false,
    refetchOnWindowFocus: false,
  });
  const overlayQuery = useQuery({
    queryKey: qk.overlay.status,
    queryFn: () => native.overlayStatus(),
    enabled: isNative(),
    staleTime: 15_000,
    retry: false,
  });

  const options = optionsQuery.data;
  const diagnosis = diagnosisQuery.data;
  const linux = Boolean(scanQuery.data?.linux);
  const gamescope = scanQuery.data?.gamescope ?? null;
  const capabilities = scanQuery.data?.displayCapabilities ?? null;
  const overlayDisabled = overlayQuery.data?.disabledGames?.includes(gameId) ?? false;
  const savedDevice = options?.env[AUDIO_DEVICE_ENV_KEY] ?? "";
  const audioDevice = audioDraft ?? savedDevice;

  const readiness = tierOneState({
    ready: diagnosis ? diagnosis.ready : null,
    platform: diagnosis?.platform ?? scanQuery.data?.platform ?? "linux",
    windowsGame: diagnosis?.windowsGame ?? true,
    mode: options?.mode ?? "auto",
    issues: diagnosis?.issues ?? [],
  });

  const applyable = useMemo(() => applyableSteps(diagnosis), [diagnosis]);
  const downloads = useMemo(() => consentDownloads(applyable), [applyable]);
  const downloadBytes = useMemo(() => totalDownloadBytes(applyable), [applyable]);
  const wrapperOn = options ? readTier2Switch({ options, overlayDisabled }, "wrapper") : false;

  /** A switch edit touches the profile only; the readiness check stays warm. */
  const invalidateProfile = async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: qk.launchOptions(gameId) }),
      queryClient.invalidateQueries({ queryKey: qk.prefixInfo(gameId) }),
      queryClient.invalidateQueries({ queryKey: qk.protondb(gameId) }),
    ]);
  };

  /** Applying a fix installs a runtime, so the diagnosis and scan are stale. */
  const invalidateAfterFix = async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: qk.makeItRun(gameId) }),
      queryClient.invalidateQueries({ queryKey: qk.compatScan }),
      queryClient.invalidateQueries({ queryKey: qk.launchOptions(gameId) }),
    ]);
  };

  const persist = async (next: LaunchOptions) => {
    const saved = await native.setLaunchOptions(gameId, next);
    queryClient.setQueryData(qk.launchOptions(gameId), saved);
    await invalidateProfile();
  };

  const toggle = async (id: Tier2SwitchId, enabled: boolean) => {
    if (!options) return;
    if (id === "overlay") {
      setSavingSwitch(id);
      try {
        const status = await native.setOverlayGameDisabled(gameId, !enabled);
        queryClient.setQueryData(qk.overlay.status, status);
        push({
          title: enabled ? t("Overlay turned on for this game") : t("Overlay turned off for this game"),
          tone: "success",
        });
      } catch (cause) {
        push({ title: t("Could not update the overlay"), description: errorMessage(cause), tone: "danger" });
      } finally {
        setSavingSwitch(null);
      }
      return;
    }
    if (id === "audioDevice" && enabled && !audioDevice.trim()) {
      setAudioEditing(true);
      return;
    }
    setSavingSwitch(id);
    try {
      await persist(withTier2Switch(options, id, enabled, audioDevice));
    } catch (cause) {
      push({ title: t("Could not save this switch"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setSavingSwitch(null);
    }
  };

  const runFix = async () => {
    if (applyable.length === 0) return;
    setFixing(true);
    try {
      const result = await native.makeItRunApply(
        gameId,
        applyable.map((step) => step.id),
        true,
      );
      setConfirmOpen(false);
      await invalidateAfterFix();
      if (result.failed) {
        push({
          title: t("The fix did not finish"),
          description: result.steps.find((step) => step.status === "failed")?.message ?? "",
          tone: "danger",
        });
      } else if (result.cancelled) {
        push({ title: t("Nothing else was installed"), tone: "default" });
      } else {
        push({
          title: result.diagnosis.ready ? t("Game fixed") : t("Step finished"),
          description: result.diagnosis.ready
            ? t("Start the game once to confirm.")
            : t("Check this game again to see what is left."),
          tone: result.diagnosis.ready ? "success" : "default",
        });
      }
    } catch (cause) {
      push({ title: t("Could not fix the game"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setFixing(false);
    }
  };

  const play = async () => {
    try {
      await native.launchGame({ gameId });
      push({ title: t("Starting the game"), tone: "success" });
    } catch (cause) {
      push({ title: t("Could not start the game"), description: errorMessage(cause), tone: "danger" });
    }
  };

  const primary = () => {
    if (readiness.status === "ready") return void play();
    if (applyable.length === 0) {
      setPlanOpen(true);
      return;
    }
    if (downloads.length > 0) setConfirmOpen(true);
    else void runFix();
  };

  const blockedReason = (id: Tier2SwitchId): string | null => {
    if (!options) return null;
    switch (id) {
      case "wrapper":
        return linux && gamescope ? null : advancedReason("noWrapper");
      case "frameLimit":
      case "hdr":
      case "adaptiveSync": {
        if (!linux) return t("This machine does not run games through a game window.");
        if (!gamescope) return advancedReason("noWrapper");
        if (!wrapperOn) return t("Turn on the game window first; this applies inside it.");
        if (id === "hdr") {
          const block = displayCapabilityBlock("hdr", gamescope, capabilities);
          return block === "output"
            ? advancedReason("hdrOutput")
            : block === "unsupportedBuild"
              ? advancedReason("hdrBuild")
              : null;
        }
        if (id === "adaptiveSync") {
          const block = displayCapabilityBlock("vrr", gamescope, capabilities);
          return block === "output"
            ? advancedReason("vrrOutput")
            : block === "unsupportedBuild"
              ? advancedReason("vrrBuild")
              : null;
        }
        return null;
      }
      case "overlay":
        return overlayQuery.isPending || overlayQuery.data
          ? null
          : t("This desktop build cannot open the overlay.");
      case "audioDevice":
        return linux ? null : t("This machine does not use the Linux audio stack.");
    }
  };

  if (!isNative()) return null;

  const switches = tier2SwitchesFor(linux);
  const statusTone = readiness.status === "ready" ? "ok" : readiness.status === "needsFix" ? "signal" : "outline";

  return (
    <section className="min-w-0" aria-labelledby="settings-tiers-heading">
      <SectionHeader
        id="settings-tiers-heading"
        level={3}
        title={settingsTiersTitle()}
        description={settingsTiersDescription()}
      />

      {/* Tier 1: computed, one action. */}
      <div className="mt-4 rounded-[var(--r-md)] border border-line bg-surface px-4 py-4">
        <div className="flex flex-wrap items-center gap-2">
          <Badge tone={statusTone}>{tierLabel("works")}</Badge>
          <p className="text-body font-medium text-ink">{tierOneTitle(readiness.status)}</p>
        </div>
        {diagnosisQuery.isError ? (
          <div className="mt-2 flex flex-wrap items-center gap-3">
            <p className="flex items-center gap-1.5 text-label text-danger">
              <AlertTriangle className="size-4 shrink-0" aria-hidden="true" />
              {t("Ludex could not check this game.")}
            </p>
            <Button
              variant="ghost"
              size="sm"
              icon={<RefreshCw className="size-3.5" />}
              onClick={() => void diagnosisQuery.refetch()}
            >
              {t("Check again")}
            </Button>
          </div>
        ) : (
          <>
            <p className="mt-1.5 max-w-[70ch] text-label leading-relaxed text-ink-2">
              {tierOneDescription(readiness.status, readiness.cause)}
            </p>
            <div className="mt-3 flex flex-wrap items-center gap-2">
              <Button
                variant="primary"
                size="sm"
                loading={fixing}
                disabled={readiness.status === "checking" || !options}
                icon={
                  readiness.status === "ready" ? (
                    <Play className="size-3.5" />
                  ) : applyable.length > 0 ? (
                    <Wand2 className="size-3.5" />
                  ) : undefined
                }
                onClick={primary}
              >
                {tierOneAction(readiness.status, applyable.length > 0)}
              </Button>
            </div>
          </>
        )}

        {readiness.status === "needsFix" && (
          <details
            className="mt-3 border-t border-line pt-3"
            open={planOpen}
            onToggle={(event) => setPlanOpen(event.currentTarget.open)}
          >
            <summary className="cursor-pointer select-none text-meta text-ink-3">
              {t("See exactly what Ludex will do")}
            </summary>
            {planOpen && (
              <div className="mt-4">
                <MakeItRunPanel gameId={gameId} />
              </div>
            )}
          </details>
        )}
      </div>

      {/* Tier 2: six curated switches, each writing one profile field. */}
      <div className="mt-8">
        <div className="flex flex-wrap items-baseline justify-between gap-2">
          <div>
            <p className="text-body font-medium text-ink">{tierLabel("game")}</p>
            <p className="mt-1 max-w-[62ch] text-meta leading-relaxed text-ink-3">
              {tier2Intro()}
            </p>
          </div>
        </div>

        <div className="mt-2 border-t border-line">
          {switches.map((meta) => {
            const checked =
              meta.id === "overlay"
                ? !overlayDisabled
                : options
                  ? readTier2Switch({ options, overlayDisabled }, meta.id)
                  : false;
            const reason = blockedReason(meta.id);
            const disabled =
              savingSwitch !== null || Boolean(reason) || (meta.id !== "overlay" && !options);
            return (
              <div key={meta.id} className="border-b border-line py-4">
                <div className="flex items-start justify-between gap-6">
                  <div className="min-w-0">
                    <p className="text-body font-medium text-ink">{tier2Label(meta.id)}</p>
                    <p className="mt-1 max-w-[62ch] text-meta leading-relaxed text-ink-3">
                      {tier2Explanation(meta.id, {
                        enabled: checked,
                        blockedReason: reason,
                        frameLimit: options?.display.refresh ?? 0,
                        device: audioDevice,
                      })}
                    </p>
                    {meta.id === "audioDevice" && (checked || audioEditing) && (
                      <div className="mt-2 flex flex-wrap items-center gap-2">
                        <Input
                          value={audioDevice}
                          onChange={(event) => setAudioDraft(event.target.value)}
                          placeholder={t("Headset or HDMI device name")}
                          aria-label={t("Audio device")}
                          spellCheck={false}
                          className="h-9 w-[240px] font-mono text-meta"
                        />
                        <Button
                          variant="ghost"
                          size="sm"
                          disabled={!audioDevice.trim() || !options || savingSwitch !== null}
                          loading={savingSwitch === "audioDevice"}
                          onClick={() => void toggle("audioDevice", true)}
                        >
                          {t("Use this device")}
                        </Button>
                      </div>
                    )}
                  </div>
                  <Switch
                    checked={checked}
                    disabled={disabled}
                    label={tier2Label(meta.id)}
                    onCheckedChange={(next) => void toggle(meta.id, next)}
                  />
                </div>
              </div>
            );
          })}
        </div>
      </div>

      {/* Tier 3: the old launch-options panel, unchanged, behind the lock. */}
      <div className="mt-8">
        {unlocked ? (
          <div className="rounded-[var(--r-md)] border border-line bg-surface px-4 py-4">
            <div className="flex flex-wrap items-start justify-between gap-3">
              <div className="min-w-0">
                <p className="flex flex-wrap items-center gap-2 text-body font-medium text-ink">
                  <Lock className="size-3.5 text-ink-3" aria-hidden="true" />
                  {tierLabel("advanced")}
                  <Badge tone="danger">{t("Unsupported")}</Badge>
                </p>
                <p className="mt-1 max-w-[70ch] text-meta leading-relaxed text-ink-3">
                  {advancedWarning()}
                </p>
              </div>
              <Button variant="ghost" size="sm" onClick={() => setAdvancedTiersUnlocked(false)}>
                {t("Lock advanced view")}
              </Button>
            </div>
          </div>
        ) : (
          <div className="rounded-[var(--r-md)] border border-line bg-surface px-4 py-4">
            <p className="flex items-center gap-2 text-body font-medium text-ink">
              <Lock className="size-3.5 text-ink-3" aria-hidden="true" />
              {t("Advanced view")}
            </p>
            <p className="mt-1 max-w-[70ch] text-meta leading-relaxed text-ink-3">
              {advancedWarning()}
            </p>
            <Button
              variant="default"
              size="sm"
              className="mt-3"
              icon={<Lock className="size-3.5" />}
              onClick={() => setAdvancedTiersUnlocked(true)}
            >
              {t("Unlock advanced view")}
            </Button>
          </div>
        )}

        {unlocked && (
          <div className="mt-6 flex flex-col gap-8">
            {isEditor && <StartupCommandPanel game={game} installed={installed} />}
            <LaunchOptionsPanel gameId={gameId} title={title} />
            <AudioOutputPanel gameId={gameId} title={title} />
            <ShaderPrepPanel gameId={gameId} title={title} />
            <LaunchPlanPanel gameId={gameId} />
            <CompatRulesPanel gameId={gameId} />
          </div>
        )}
      </div>

      <Dialog
        open={confirmOpen}
        onOpenChange={setConfirmOpen}
        title={t("Fix this game")}
        description={t("Ludex will install what this game needs. Nothing is downloaded or installed without this click.")}
        footer={
          <>
            <Button variant="ghost" size="sm" onClick={() => setConfirmOpen(false)}>
              {t("Cancel")}
            </Button>
            <Button variant="primary" size="sm" loading={fixing} onClick={() => void runFix()}>
              {t("Install and fix")}
            </Button>
          </>
        }
      >
        <ul className="divide-y divide-line border-y border-line">
          {downloads.map((step) => (
            <li key={step.id} className="flex flex-wrap items-baseline justify-between gap-2 py-2">
              <span className="font-mono text-meta text-ink-2">{step.download?.host}</span>
              <span className="num text-meta text-ink-3">
                {step.download ? formatBytes(step.download.sizeBytes) : ""}
              </span>
              <span className="text-meta text-ink-3">{step.download?.licence}</span>
            </li>
          ))}
        </ul>
        {downloadBytes > 0 && (
          <p className="mt-2 text-meta text-ink-3">
            {t("Total download: {size}", { size: formatBytes(downloadBytes) })}
          </p>
        )}
      </Dialog>
    </section>
  );
}
