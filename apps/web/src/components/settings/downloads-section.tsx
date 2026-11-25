import { useEffect, useState } from "react";
import { Link } from "react-router-dom";
import { ChevronRight } from "lucide-react";
import { kibToMbps, mbpsToKib } from "@/lib/format";
import { setAdvancedTiersUnlocked, useAdvancedTiersUnlocked } from "@/lib/settings-tiers";
import { t, tn, tr } from "@/lib/i18n";
import { Button, Input, Textarea } from "@/components/ui/controls";
import { Select } from "@/components/ui/select";
import {
  DesktopSaveBar,
  SettingsGroup,
  SettingsSwitchRow,
} from "@/components/settings/settings-section";
import { DesktopBody } from "@/components/settings/sections-device";
import type { DesktopSettingsController } from "@/components/settings/use-desktop-settings";

/** The `downloadPolicy` options (#235): automatic first, overrides last. */
const DOWNLOAD_POLICY_OPTIONS = [
  { value: "automatic", label: t("Automatic (recommended)") },
  { value: "stream", label: t("Stream") },
  { value: "classic", label: t("Classic archive") },
] as const;

/** Downloads: everyday controls by default, deep tuning behind Power-user mode (#347). */
export function DownloadsBody({ desktop }: { desktop: DesktopSettingsController }) {
  const unlocked = useAdvancedTiersUnlocked();
  return (
    <DesktopBody desktop={desktop}>
      {(draft) => (
        <>
          <SettingsGroup title={t("Installs")}>
            <SettingsSwitchRow
              title={t("Keep downloaded archives")}
              description={t("Leave the archive in the download folder after a successful install. Off by default; the desktop deletes it to save space.")}
              checked={draft.keepArchives}
              onCheckedChange={(keepArchives) => desktop.update({ keepArchives })}
            />
          </SettingsGroup>

          <SettingsGroup title={t("After a restart")}>
            <SettingsSwitchRow
              title={t("Resume unfinished downloads on launch")}
              description={t("Re-enters an install that was still downloading when Ludex closed, at the same offset and in the same queue position. A paused or cancelled install is never resumed automatically.")}
              checked={draft.autoResumeDownloads}
              onCheckedChange={(autoResumeDownloads) => desktop.update({ autoResumeDownloads })}
            />
          </SettingsGroup>

          <SettingsGroup
            title={t("Bandwidth limit")}
            description={t("Caps every download (installs, repairs and stream installs) in Mb/s. Use 0 for no limit.")}
          >
            <BandwidthLimitField
              kib={draft.bandwidthLimitKib}
              onChange={(bandwidthLimitKib) => desktop.update({ bandwidthLimitKib })}
            />
          </SettingsGroup>

          {!unlocked ? (
            <SettingsGroup
              title={t("Advanced download tuning")}
              description={t("Streaming policy, local file cache, staging drive, retries, shader prep, concurrency, archive passwords and silent installer flags use safe automatic defaults.")}
            >
              <Button variant="default" size="sm" onClick={() => setAdvancedTiersUnlocked(true)}>
                {t("Show advanced download options")}
              </Button>
            </SettingsGroup>
          ) : (
            <>
              <SettingsGroup title={t("Transfer strategy")}>
                <SettingsSwitchRow
                  title={t("Stream installs from the server")}
                  description={t("ZIP archives only. Files are written as they arrive, so the archive never touches the disk. A dropped connection, a pause or an app restart resumes at file granularity: completed files are kept and only missing ones are downloaded again. 7z/RAR archives, and servers without the per-entry API, fall back to the normal resumable download.")}
                  checked={draft.streamInstall}
                  onCheckedChange={(streamInstall) => desktop.update({ streamInstall })}
                />
                <SettingsGroup
                  title={t("Download policy")}
                  description={t("How a version reaches this device. Automatic picks the fastest path the server supports; Stream and Classic pin one path for troubleshooting.")}
                >
                  <div className="max-w-[280px]">
                    <Select
                      label={t("Download policy")}
                      value={draft.downloadPolicy}
                      onChange={(value) => desktop.update({ downloadPolicy: value })}
                      options={[...DOWNLOAD_POLICY_OPTIONS]}
                      className="h-10 text-label"
                    />
                  </div>
                  <p className="mt-3 max-w-[54ch] text-meta leading-relaxed text-ink-3">
                    {draft.downloadPolicy === "automatic"
                      ? t("Resolved per version: the server's extracted files when they are ready, then a streamed ZIP, then the archive. A probe that finds nothing falls back on its own; only a transfer that fails mid-install says why on the job.")
                      : draft.downloadPolicy === "stream"
                        ? t("Streams ZIP entries as they arrive and never stores the archive. 7z and RAR archives still download and extract the archive.")
                        : t("Always downloads the archive, verifies its SHA-256, then extracts it. Works with every format and every server build.")}
                  </p>
                </SettingsGroup>
              </SettingsGroup>

              <SettingsGroup
                title={t("Server file cache")}
                description={t("In local mode the embedded server can extract an archive once and serve the files to this device, so an install downloads files instead of an archive. It runs on this machine and is bounded by the budget below. The change applies the next time the local server starts.")}
              >
                <SettingsSwitchRow
                  title={t("Serve extracted files to installs")}
                  description={t("On by default in local mode. Off (or full) keeps the archive download and never writes the cache; a remote server keeps its own LUDEX_EXTRACTED_CACHE opt-in.")}
                  checked={draft.serverFileCache}
                  onCheckedChange={(serverFileCache) => desktop.update({ serverFileCache })}
                />
                <div className="mt-4 flex flex-wrap items-center gap-2">
                  <div className="w-[150px]">
                    <Input
                      id="server-file-cache-budget"
                      aria-label={t("Server file cache budget in GB")}
                      type="number"
                      min={1}
                      max={1000}
                      step={1}
                      inputMode="numeric"
                      disabled={!draft.serverFileCache}
                      value={draft.serverFileCacheBudgetGb}
                      onChange={(event) =>
                        desktop.update({
                          serverFileCacheBudgetGb: Math.min(1000, Math.max(1, Math.round(Number(event.target.value) || 50))),
                        })
                      }
                      className="font-mono text-label"
                    />
                  </div>
                  <span className="text-meta text-ink-3" aria-hidden="true">
                    {t("GB")}
                  </span>
                </div>
                <p className="mt-2 max-w-[54ch] text-meta leading-relaxed text-ink-3">
                  {draft.serverFileCache
                    ? t("Entries unused for 14 days are evicted, and the least recently used ones go first when the budget is reached. The cache lives next to the local server's data and is never unbounded.")
                    : t("The cache stays off; installs download the archive and extract it locally.")}
                </p>
              </SettingsGroup>

              <SettingsGroup
                title={t("Download staging")}
                description={t("Where the archive is written while it downloads. The install dialog and the disk-space check use the same choice.")}
              >
                <div role="radiogroup" aria-label={t("Download staging")} className="flex flex-col gap-4">
                  <label className="flex cursor-pointer items-start gap-3">
                    <input
                      type="radio"
                      name="download-staging"
                      value="configured"
                      checked={draft.downloadStaging === "configured"}
                      onChange={() => desktop.update({ downloadStaging: "configured", downloadStagingConfirmed: true })}
                      className="mt-0.5 size-4 shrink-0 accent-signal"
                    />
                    <span className="min-w-0">
                      <span className="block text-body font-medium text-ink">{t("Configured download folder")}</span>
                      <span className="mt-1 block max-w-[54ch] text-meta leading-relaxed text-ink-3">
                        {t("The archive is written to the download folder, verified there, then extracted into the install location.")}
                      </span>
                    </span>
                  </label>
                  <label className="flex cursor-pointer items-start gap-3">
                    <input
                      type="radio"
                      name="download-staging"
                      value="installVolume"
                      checked={draft.downloadStaging === "installVolume"}
                      onChange={() => desktop.update({ downloadStaging: "installVolume", downloadStagingConfirmed: true })}
                      className="mt-0.5 size-4 shrink-0 accent-signal"
                    />
                    <span className="min-w-0">
                      <span className="block text-body font-medium text-ink">{t("Install drive (default)")}</span>
                      <span className="mt-1 block max-w-[54ch] text-meta leading-relaxed text-ink-3">
                        {t("The archive is staged next to the game, so hashing and extraction never read it back over the network. It needs temporary free space on the install drive, and the archive is deleted after extraction unless Keep downloaded archives is on.")}
                      </span>
                    </span>
                  </label>
                </div>
              </SettingsGroup>

              <SettingsGroup
                title={t("Connection recovery")}
                description={t("Ludex aborts a response that stays silent for this many seconds and resumes from the last received byte. A transfer that keeps receiving data is never interrupted.")}
              >
                <div className="flex flex-wrap items-center gap-2">
                  <div className="w-[150px]">
                    <Input
                      id="stall-timeout"
                      aria-label={t("Connection recovery")}
                      type="number"
                      min={5}
                      max={600}
                      step={5}
                      inputMode="numeric"
                      value={draft.downloadStallTimeoutSecs}
                      onChange={(event) =>
                        desktop.update({
                          downloadStallTimeoutSecs: Math.min(600, Math.max(5, Math.round(Number(event.target.value) || 30))),
                        })
                      }
                      className="font-mono text-label"
                    />
                  </div>
                  <span className="text-meta text-ink-3" aria-hidden="true">
                    s
                  </span>
                </div>
              </SettingsGroup>

              <SettingsGroup
                title={t("Automatic retries")}
                description={t("A dropped transfer retries on its own for this long before the install stops: same job, same offset, with a backoff capped at five minutes. A wrong password, a missing game or a full disk stops immediately instead of looping. Use 0 to keep retrying until the app closes.")}
              >
                <div className="flex flex-wrap items-center gap-2">
                  <div className="w-[150px]">
                    <Input
                      id="retry-window"
                      aria-label={t("Retry window in hours")}
                      type="number"
                      min={0}
                      max={168}
                      step={1}
                      inputMode="numeric"
                      value={Math.round(draft.downloadRetryWindowSecs / 3600)}
                      onChange={(event) => {
                        const hours = Math.max(0, Math.min(168, Number(event.target.value) || 0));
                        desktop.update({ downloadRetryWindowSecs: Math.round(hours * 3600) });
                      }}
                      className="font-mono text-label"
                    />
                  </div>
                  <span className="text-meta text-ink-3" aria-hidden="true">
                    h
                  </span>
                </div>
                <p className="mt-2 text-meta leading-relaxed text-ink-3">
                  {draft.downloadRetryWindowSecs === 0
                    ? t("Retrying until the app closes. Cancel always stops the download immediately.")
                    : tn(
                        Math.round(draft.downloadRetryWindowSecs / 3600),
                        "Retrying for up to 1 hour. Cancel always stops the download immediately.",
                        "Retrying for up to {count} hours. Cancel always stops the download immediately.",
                      )}
                </p>
              </SettingsGroup>

              <SettingsGroup
                title={t("Keep the machine awake")}
                description={t("While a download, a game or a stream is running, Ludex blocks suspend and display blanking. The inhibitor is released after this many minutes; 0 releases it as soon as the last job ends. Bounded 0..60.")}
              >
                <div className="flex flex-wrap items-center gap-2">
                  <div className="w-[150px]">
                    <Input
                      id="keep-awake-after-minutes"
                      aria-label={t("Minutes to keep the machine awake after the last job")}
                      type="number"
                      min={0}
                      max={60}
                      step={1}
                      inputMode="numeric"
                      value={draft.keepAwakeAfterMinutes}
                      onChange={(event) =>
                        desktop.update({
                          keepAwakeAfterMinutes: Math.min(60, Math.max(0, Math.round(Number(event.target.value) || 0))),
                        })
                      }
                      className="font-mono text-label"
                    />
                  </div>
                  <span className="text-meta text-ink-3" aria-hidden="true">
                    {t("min")}
                  </span>
                </div>
              </SettingsGroup>

              <SettingsGroup
                title={t("Shader preparation")}
                description={t("Runs a game once after it installs to fill its DXVK, VKD3D-Proton and Mesa caches, then stops it once the caches stop growing. No reliable headless mode exists, so a window may appear during the run. It never blocks the first launch and a failure leaves the game untouched.")}
              >
                <SettingsSwitchRow
                  title={t("Prepare after install")}
                  description={t("Applies to every game without its own choice on the game page.")}
                  checked={draft.shaderPrepAfterInstall}
                  onCheckedChange={(checked) => desktop.update({ shaderPrepAfterInstall: checked })}
                />
                <div className="mt-4 flex flex-wrap items-center gap-2">
                  <div className="w-[150px]">
                    <Input
                      id="shader-prep-budget"
                      aria-label={t("Time budget in seconds")}
                      type="number"
                      min={30}
                      max={900}
                      step={15}
                      inputMode="numeric"
                      value={draft.shaderPrepBudgetSecs}
                      onChange={(event) =>
                        desktop.update({
                          shaderPrepBudgetSecs: Math.min(900, Math.max(30, Math.round(Number(event.target.value) || 180))),
                        })
                      }
                      className="font-mono text-label"
                    />
                  </div>
                  <span className="text-meta text-ink-3" aria-hidden="true">
                    {t("seconds")}
                  </span>
                </div>
              </SettingsGroup>

              <SettingsGroup
                title={t("Parallel downloads")}
                description={t("How many archives may download at once. One is usually enough: Ludex already downloads the next game while the current one extracts. Extraction is always one at a time, and every download is only started when its disk reservation fits.")}
              >
                <div className="flex flex-wrap items-center gap-2">
                  <div className="w-[150px]">
                    <Input
                      id="download-concurrency"
                      aria-label={t("Parallel downloads")}
                      type="number"
                      min={1}
                      max={2}
                      step={1}
                      inputMode="numeric"
                      value={draft.downloadConcurrency}
                      onChange={(event) =>
                        desktop.update({
                          downloadConcurrency: Math.min(2, Math.max(1, Math.round(Number(event.target.value) || 1))),
                        })
                      }
                      className="font-mono text-label"
                    />
                  </div>
                  <span className="text-meta text-ink-3" aria-hidden="true">
                    {t("at a time")}
                  </span>
                </div>
                <p className="mt-2 text-meta leading-relaxed text-ink-3">
                  {draft.downloadConcurrency > 1
                    ? t("Two archives may download in parallel while one extracts. The disk reservation still gates every start, so this never overcommits the same volume.")
                    : t("One archive downloads while the previous one extracts: the network is not left idle waiting for an unpack.")}
                </p>
              </SettingsGroup>

              <SettingsGroup title={t("Encrypted archives")}>
                <label htmlFor="archive-passwords" className="text-body font-medium text-ink">
                  {t("Archive passwords")}
                </label>
                <p className="mb-2 mt-1 max-w-[54ch] text-meta leading-relaxed text-ink-3">
                  {t("Tried in order when an archive is encrypted. One per line, or separated by commas.")}
                </p>
                <Textarea
                  id="archive-passwords"
                  value={draft.archivePasswords.join("\n")}
                  onChange={(event) =>
                    desktop.update({
                      archivePasswords: event.target.value
                        .split(/[\n,]+/)
                        .map((entry) => entry.trim())
                        .filter(Boolean),
                    })
                  }
                  placeholder={"first-password\nsecond-password"}
                  rows={3}
                  className="min-h-[84px] font-mono text-label"
                />
                <p className="mt-1 text-meta text-ink-3">
                  {draft.archivePasswords.length === 0
                    ? t("No passwords. Encrypted archives will fail to extract.")
                    : tn(draft.archivePasswords.length, "1 password stored locally in the desktop settings.", "{count} passwords stored locally in the desktop settings.")}
                </p>
              </SettingsGroup>

              <SettingsGroup title={t("Quiet installers")}>
                <p className="text-body font-medium text-ink">{t("Silent installer arguments")}</p>
                <p className="mb-2 mt-1 max-w-[54ch] text-meta leading-relaxed text-ink-3">
                  {tr("Tried in order when a game ships a setup program, one argument set per line. Leave empty for the built-in presets ({presets}).", {
                    presets: (
                      <>
                        <span className="font-mono">/S</span>, <span className="font-mono">/silent</span>,{" "}
                        <span className="font-mono">/VERYSILENT /SUPPRESSMSGBOXES /NORESTART</span>,{" "}
                        <span className="font-mono">--silent</span>
                      </>
                    ),
                  })}
                </p>
                <Textarea
                  id="installer-args"
                  value={draft.installerSilentArgs.map((set) => set.join(" ")).join("\n")}
                  onChange={(event) =>
                    desktop.update({
                      installerSilentArgs: event.target.value
                        .split("\n")
                        .map((line) => line.trim())
                        .filter(Boolean)
                        .map((line) => line.split(/\s+/)),
                    })
                  }
                  placeholder={"/S\n/VERYSILENT /SUPPRESSMSGBOXES /NORESTART"}
                  rows={3}
                  className="min-h-[78px] font-mono text-label"
                />
                <div className="mt-3 flex flex-wrap items-center gap-2">
                  <label htmlFor="installer-timeout" className="text-label text-ink-2">
                    {t("Timeout per attempt")}
                  </label>
                  <div className="w-[110px]">
                    <Input
                      id="installer-timeout"
                      type="number"
                      min={30}
                      max={7200}
                      step={30}
                      inputMode="numeric"
                      value={draft.installerTimeoutSecs}
                      onChange={(event) =>
                        desktop.update({ installerTimeoutSecs: Math.max(30, Math.round(Number(event.target.value) || 600)) })
                      }
                      className="font-mono text-label"
                    />
                  </div>
                  <span className="text-meta text-ink-3">{t("seconds")}</span>
                </div>
              </SettingsGroup>
            </>
          )}

          <div className="mt-6 flex flex-wrap items-center gap-3 border-t border-line pt-4">
            <span className="text-meta text-ink-3">{t("Finished downloads, history and the queue live on their own screen.")}</span>
            <Link
              to="/downloads"
              className="inline-flex h-11 items-center gap-1 text-label text-ink-2 underline-offset-4 hover:text-ink hover:underline"
            >
              {t("Open the downloads screen")}
              <ChevronRight className="rtl:-scale-x-100 size-3" aria-hidden="true" />
            </Link>
          </div>

          <DesktopSaveBar desktop={desktop} />
        </>
      )}
    </DesktopBody>
  );
}

/**
 * Bandwidth cap, stored in KiB/s but edited in Mb/s (#199). The text stays
 * local while typing so a partially typed value ("8.") does not bounce back.
 */
function BandwidthLimitField({ kib, onChange }: { kib: number; onChange: (kib: number) => void }) {
  const [text, setText] = useState(() => (kib > 0 ? formatMbpsInput(kib) : "0"));
  useEffect(() => {
    const parsed = Number(text);
    const stored = Number.isFinite(parsed) ? mbpsToKib(parsed) : 0;
    if (stored !== kib) setText(kib > 0 ? formatMbpsInput(kib) : "0");
    // Resync only when the saved cap changes from outside this input.
  }, [kib]);
  return (
    <div className="flex flex-wrap items-center gap-2">
      <div className="w-[150px]">
        <Input
          id="bandwidth-limit"
          aria-label={t("Bandwidth limit")}
          type="number"
          min={0}
          step={1}
          inputMode="decimal"
          value={text}
          onChange={(event) => {
            const value = event.target.value;
            setText(value);
            onChange(mbpsToKib(Number(value) || 0));
          }}
          className="font-mono text-label"
        />
      </div>
      <span className="text-meta text-ink-3" aria-hidden="true">
        Mb/s
      </span>
      {kib === 0 && <span className="num text-meta text-ink-3">{t("Unlimited")}</span>}
    </div>
  );
}

/** Whole or one-decimal Mb/s without a trailing `.0`. */
function formatMbpsInput(kib: number): string {
  return String(Math.round(kibToMbps(kib) * 10) / 10);
}
