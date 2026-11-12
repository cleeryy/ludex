import { useEffect, useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { FolderOpen, RotateCcw, TerminalSquare } from "lucide-react";
import { errorMessage } from "@/lib/http";
import { isNative, native } from "@/lib/native";
import { formatBytes , formatDuration} from "@/lib/format";
import type { CompatTool, LaunchMode, LaunchOptions, WinetricksReport } from "@/lib/types";
import { useToast } from "@/state/toasts";
import { Badge, Button, Input, Textarea } from "@/components/ui/controls";
import { Select } from "@/components/ui/select";
import { SectionHeader } from "@/components/page";
import { cn } from "@/lib/cn";
import { tr, t } from "@/lib/i18n";

function modeOptions(): { value: LaunchMode; label: string; hint: string }[] {
  return [
    { value: "auto", label: t("Auto"), hint: t("Native files run directly; .exe through the best tool") },
    { value: "native", label: t("Native"), hint: t("Run the file directly, no compatibility layer") },
    { value: "proton", label: t("Proton"), hint: t("Proton run game.exe") },
    { value: "umu", label: t("UMU"), hint: t("umu-run, then a Proton build") },
  ];
}

const EMPTY_OPTIONS: LaunchOptions = {
  mode: "auto",
  toolId: "",
  prefixPath: "",
  env: {},
  winetricksVerbs: [],
  steamAppId: null,
  workingDir: "",
};

function envToText(env: Record<string, string>): string {
  return Object.entries(env)
    .map(([key, value]) => `${key}=${value}`)
    .join("\n");
}

function textToEnv(text: string): Record<string, string> {
  const env: Record<string, string> = {};
  for (const raw of text.split("\n")) {
    const line = raw.trim();
    if (!line || line.startsWith("#")) continue;
    const split = line.indexOf("=");
    if (split <= 0) continue;
    const key = line.slice(0, split).trim();
    if (!key) continue;
    env[key] = line.slice(split + 1).trim();
  }
  return env;
}

/**
 * Per-game launch options (#89, #92, #187).
 *
 * Linux: mode, tool, prefix, environment overrides, winetricks verbs and the
 * manual Steam app id used by the ProtonDB badge. Every platform: the working
 * directory override, which fixes adopted Windows games that load their data
 * relative to the executable's folder.
 */
export function LaunchOptionsPanel({ gameId, title }: { gameId: number; title: string }) {
  const { push } = useToast();
  const queryClient = useQueryClient();

  const scan = useQuery({
    queryKey: ["compat-scan"],
    queryFn: () => native.compatScan(),
    enabled: isNative(),
    staleTime: 60_000,
    retry: false,
  });
  const savedQuery = useQuery({
    queryKey: ["launch-options", gameId],
    queryFn: () => native.getLaunchOptions(gameId),
    enabled: isNative(),
    retry: false,
  });
  const linux = Boolean(scan.data?.linux);
  const prefixQuery = useQuery({
    queryKey: ["prefix-info", gameId],
    queryFn: () => native.winePrefixInfo(gameId),
    enabled: isNative() && linux && Boolean(savedQuery.data),
    staleTime: 20_000,
    retry: false,
  });

  const [draft, setDraft] = useState<LaunchOptions>(EMPTY_OPTIONS);
  const [envText, setEnvText] = useState("");
  const [customVerb, setCustomVerb] = useState("");
  const [saving, setSaving] = useState(false);
  const [resetting, setResetting] = useState(false);
  const [confirmReset, setConfirmReset] = useState(false);
  const [runningVerbs, setRunningVerbs] = useState(false);
  const [verbReport, setVerbReport] = useState<WinetricksReport | null>(null);

  useEffect(() => {
    if (!savedQuery.data) return;
    setDraft(savedQuery.data);
    setEnvText(envToText(savedQuery.data.env));
  }, [savedQuery.data]);

  const tools = scan.data?.tools ?? [];
  const verbs = scan.data?.suggestedVerbs ?? [];
  const compatibleTools = useMemo(() => {
    if (draft.mode === "native") return [] as CompatTool[];
    if (draft.mode === "umu") return tools.filter((tool) => tool.kind === "umu");
    if (draft.mode === "proton") return tools.filter((tool) => tool.kind === "proton");
    return tools;
  }, [draft.mode, tools]);

  if (!isNative()) return null;

  const update = (patch: Partial<LaunchOptions>) => setDraft((current) => ({ ...current, ...patch }));

  const dirty = Boolean(savedQuery.data && JSON.stringify(draft) !== JSON.stringify(savedQuery.data));
  const savedOptions = savedQuery.data ?? EMPTY_OPTIONS;
  const savedPrefix = prefixQuery.data;

  const save = async () => {
    setSaving(true);
    try {
      const next = await native.setLaunchOptions(gameId, { ...draft, env: textToEnv(envText) });
      setDraft(next);
      setEnvText(envToText(next.env));
      await queryClient.invalidateQueries({ queryKey: ["launch-options", gameId] });
      await queryClient.invalidateQueries({ queryKey: ["prefix-info", gameId] });
      await queryClient.invalidateQueries({ queryKey: ["protondb", gameId] });
      push({
        title: t("Linux options saved"),
        description: t("{title} will start with the {profile} profile.", { title, profile: modeOptions().find((option) => option.value === next.mode)?.label ?? next.mode }),
        tone: "success",
      });
    } catch (cause) {
      push({ title: t("Could not save the launch options"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setSaving(false);
    }
  };

  const pickPrefix = async () => {
    try {
      const path = await native.pickFolder("Choose a Wine prefix folder");
      if (path) update({ prefixPath: path });
    } catch (cause) {
      push({ title: t("Could not open the folder picker"), description: errorMessage(cause), tone: "danger" });
    }
  };

  const pickWorkingDir = async () => {
    try {
      const path = await native.pickFolder(t("Choose a folder"));
      if (path) update({ workingDir: path });
    } catch (cause) {
      push({ title: t("Could not open the folder picker"), description: errorMessage(cause), tone: "danger" });
    }
  };

  const resetPrefix = async () => {
    setResetting(true);
    try {
      await native.resetWinePrefix(gameId);
      setConfirmReset(false);
      await prefixQuery.refetch();
      push({ title: t("Wine prefix deleted"), description: t("It is recreated empty on the next launch."), tone: "success" });
    } catch (cause) {
      push({ title: t("Could not delete the prefix"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setResetting(false);
    }
  };

  const toggleVerb = (verb: string) => {
    const current = draft.winetricksVerbs;
    update({
      winetricksVerbs: current.includes(verb)
        ? current.filter((entry) => entry !== verb)
        : [...current, verb],
    });
  };

  const addVerb = () => {
    const verb = customVerb.trim().toLowerCase();
    if (!verb) return;
    if (!draft.winetricksVerbs.includes(verb)) {
      update({ winetricksVerbs: [...draft.winetricksVerbs, verb] });
    }
    setCustomVerb("");
  };

  const runVerbs = async () => {
    if (draft.winetricksVerbs.length === 0) return;
    setRunningVerbs(true);
    setVerbReport(null);
    try {
      const report = await native.runWinetricks(gameId, draft.winetricksVerbs);
      setVerbReport(report);
      push({
        title: report.success ? "Winetricks finished" : "Winetricks needs attention",
        description: report.success
          ? `${report.verbs.join(", ")} installed in the game prefix.`
          : report.timedOut
            ? "Winetricks timed out after 30 minutes."
            : `Exit code ${report.exitCode ?? "unknown"}. See the log below.`,
        tone: report.success ? "success" : "danger",
      });
    } catch (cause) {
      push({ title: t("Could not run winetricks"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setRunningVerbs(false);
    }
  };

  const toolHint =
    draft.mode === "native"
      ? "No tool: the file runs as-is"
      : compatibleTools.length === 0
        ? "No matching tool detected"
        : `${compatibleTools.length} available`;

  return (
    <div className="min-w-0">
      <SectionHeader
        level={3}
        title={linux ? t("Linux launch options") : t("Launch options")}
        description={
          linux
            ? t("Proton/UMU tool, Wine prefix and environment")
            : t("Used by the desktop app when it starts the game")
        }
        actions={
          linux && draft.steamAppId !== null ? (
            <a
              href={`https://www.protondb.com/app/${draft.steamAppId}`}
              target="_blank"
              rel="noreferrer noopener"
              className="text-[11.5px] text-ink-3 underline-offset-4 hover:text-ink hover:underline"
            >
              {t("ProtonDB")}
            </a>
          ) : undefined
        }
      />
      <div className="border-t border-line pt-4">
        {linux && scan.data && scan.data.tools.length === 0 && (
          <p className="mb-4 border-l-2 border-signal/60 pl-3 text-[12.5px] leading-relaxed text-ink-2">
            {tr("No compatibility tool is installed. A Windows game cannot start until UMU, GE-Proton/Proton or Wine is available. Install one, then rescan in {settings}.", {
              settings: (
                <Link to="/settings" className="underline underline-offset-4 hover:text-ink">
                  {t("Settings → Compatibility")}
                </Link>
              ),
            })}
          </p>
        )}

        {linux && (
          <>
            <div className="grid gap-5 sm:grid-cols-2">
              <label className="flex flex-col gap-1.5">
                <span className="text-[12px] text-ink-3">{t("Launch mode")}</span>
                <Select
                  label={t("Launch mode")}
                  value={draft.mode}
                  onChange={(mode) => update({ mode })}
                  options={modeOptions()}
                />
              </label>

          <label className="flex flex-col gap-1.5">
            <span className="text-[12px] text-ink-3">{t("Compatibility tool")}</span>
            <Select
              label={t("Compatibility tool")}
              value={draft.mode === "native" ? "" : draft.toolId}
              onChange={(toolId) => update({ toolId })}
              disabled={draft.mode === "native"}
              options={[
                { value: "", label: t("Best available"), hint: toolHint },
                ...compatibleTools.map((tool) => ({
                  value: tool.id,
                  label: tool.version ? `${tool.name} · ${tool.version}` : tool.name,
                  hint: `${tool.kind} · ${tool.source}`,
                })),
              ]}
            />
            <span className="text-[11px] text-ink-3">
              {scan.data?.umu
                ? t("UMU is preferred, then GE-Proton/Proton, then Wine.")
                : t("GE-Proton/Proton is preferred, then Wine.")}
            </span>
          </label>

          <label className="flex flex-col gap-1.5">
            <span className="text-[12px] text-ink-3">{t("Steam app id")}</span>
            <Input
              value={draft.steamAppId ?? ""}
              onChange={(event) => {
                const value = event.target.value.trim();
                update({ steamAppId: value ? Number(value) || null : null });
              }}
              inputMode="numeric"
              placeholder="1145360"
              spellCheck={false}
              className="h-10 font-mono text-[12px]"
            />
            <span className="text-[11px] text-ink-3">{t("Used for the ProtonDB rating and `GAMEID`. Optional.")}</span>
          </label>

          <div className="flex flex-col gap-1.5">
            <span className="text-[12px] text-ink-3">{t("Environment overrides")}</span>
            <Textarea
              value={envText}
              onChange={(event) => setEnvText(event.target.value)}
              placeholder={"DXVK_HUD=fps\nPROTON_NO_ESYNC=1\nWINEDLLOVERRIDES=d3d11=n"}
              rows={3}
              spellCheck={false}
              className="min-h-[84px] font-mono text-[11.5px]"
            />
            <span className="text-[11px] text-ink-3">{t("One KEY=VALUE per line. PATH, HOME and LD_* are refused.")}</span>
          </div>
        </div>

        <div className="mt-6 border-t border-line pt-4">
          <div className="flex flex-wrap items-baseline justify-between gap-3">
            <p className="text-[13px] font-medium text-ink">{t("Wine prefix")}</p>
            <span className="font-mono text-[10.5px] text-ink-3">
              {savedPrefix?.exists
                ? `${formatBytes(savedPrefix.sizeBytes)} · ${savedPrefix.custom ? "custom" : "default"}`
                : t("not created yet")}
            </span>
          </div>
          <p className="mt-1 max-w-[70ch] text-[11.5px] leading-relaxed text-ink-3">
            {tr("Each game gets its own prefix. The default lives inside the install folder ({prefix}) and is recreated on the next launch after a reset. Proton uses {pfx} inside it.", {
              prefix: <span className="font-mono">.prefix</span>,
              pfx: <span className="font-mono">pfx</span>,
            })}
          </p>
          <div className="mt-3 flex flex-col gap-2 sm:flex-row sm:items-center">
            <Input
              value={draft.prefixPath}
              onChange={(event) => update({ prefixPath: event.target.value })}
              placeholder={savedPrefix?.path ?? "/path/to/games/.prefix"}
              spellCheck={false}
              className="h-10 flex-1 font-mono text-[11.5px]"
            />
            <div className="flex shrink-0 items-center gap-2">
              <Button variant="default" size="sm" className="h-10" icon={<FolderOpen className="size-3.5" />} onClick={() => void pickPrefix()}>
                {t("Browse")}
              </Button>
              {confirmReset ? (
                <>
                  <Button variant="danger" size="sm" className="h-10" loading={resetting} onClick={() => void resetPrefix()}>
                    {t("Confirm delete")}
                  </Button>
                  <Button variant="ghost" size="sm" className="h-10" onClick={() => setConfirmReset(false)}>
                    {t("Cancel")}
                  </Button>
                </>
              ) : (
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-10 text-ink-3"
                  icon={<RotateCcw className="size-3.5" />}
                  disabled={!savedPrefix?.exists}
                  onClick={() => setConfirmReset(true)}
                >
                  {t("Reset prefix")}
                </Button>
              )}
            </div>
          </div>
          {draft.prefixPath !== savedOptions.prefixPath && (
            <p className="mt-1.5 text-[11px] text-ink-3">{t("Save to apply the new prefix path.")}</p>
          )}
        </div>

        <div className="mt-6 border-t border-line pt-4">
          <p className="text-[13px] font-medium text-ink">{t("Winetricks verbs")}</p>
          <p className="mt-1 max-w-[70ch] text-[11.5px] leading-relaxed text-ink-3">
            {t("Installed on demand in this game prefix, with the Proton Wine build when one is selected. Runs can take several minutes.")}
          </p>
          <div className="mt-3 flex flex-wrap gap-1.5">
            {verbs.map((verb) => {
              const active = draft.winetricksVerbs.includes(verb);
              return (
                <button
                  key={verb}
                  type="button"
                  onClick={() => toggleVerb(verb)}
                  aria-pressed={active}
                  className={cn(
                    "h-7 border px-2 font-mono text-[10.5px] transition-colors",
                    active
                      ? "border-signal/60 bg-signal/10 text-signal-text"
                      : "border-line-2 text-ink-3 hover:border-line-2 hover:text-ink",
                  )}
                >
                  {verb}
                </button>
              );
            })}
          </div>
          <div className="mt-3 flex flex-wrap items-center gap-2">
            <Input
              value={customVerb}
              onChange={(event) => setCustomVerb(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") {
                  event.preventDefault();
                  addVerb();
                }
              }}
              placeholder="another-verb"
              spellCheck={false}
              className="h-9 w-[190px] font-mono text-[11.5px]"
            />
            <Button variant="ghost" size="sm" onClick={addVerb} disabled={!customVerb.trim()}>
              {t("Add")}
            </Button>
            <Button
              variant="default"
              size="sm"
              icon={<TerminalSquare className="size-3.5" />}
              loading={runningVerbs}
              disabled={draft.winetricksVerbs.length === 0 || !scan.data?.winetricks}
              onClick={() => void runVerbs()}
            >
              {t("Run winetricks")}
            </Button>
            {draft.winetricksVerbs.length > 0 && (
              <span className="font-mono text-[10.5px] text-ink-3">
                {draft.winetricksVerbs.join(" ")}
              </span>
            )}
            {!scan.data?.winetricks && (
              <span className="text-[11px] text-ink-3">{t("winetricks is not installed.")}</span>
            )}
          </div>

          {verbReport && (
            <div className="mt-3 border-l-2 border-line-2 pl-3">
              <div className="flex flex-wrap items-baseline gap-2">
                <Badge tone={verbReport.success ? "ok" : verbReport.timedOut ? "signal" : "danger"}>
                  {verbReport.success ? t("Done") : verbReport.timedOut ? t("Timed out") : t("Failed")}
                </Badge>
                {verbReport.exitCode !== null && (
                  <span className="num text-[11px] text-ink-3">{t("exit")} {verbReport.exitCode}</span>
                )}
                <span className="num text-[11px] text-ink-3">
                  {formatDuration(verbReport.durationMs / 1000)}
                </span>
              </div>
              {verbReport.output && (
                <pre className="mt-2 max-h-40 overflow-auto whitespace-pre-wrap border border-line bg-surface p-2 font-mono text-[10.5px] leading-relaxed text-ink-3">
                  {verbReport.output}
                </pre>
              )}
            </div>
          )}
        </div>
          </>
        )}

        <div className="mt-6 border-t border-line pt-4">
          <p className="text-[13px] font-medium text-ink">{t("Working directory")}</p>
          <p className="mt-1 max-w-[70ch] text-[11.5px] leading-relaxed text-ink-3">
            {t("Folder the game starts in. Empty uses the install folder; an adopted game otherwise starts in its executable's folder.")}
          </p>
          <div className="mt-3 flex flex-col gap-2 sm:flex-row sm:items-center">
            <Input
              value={draft.workingDir}
              onChange={(event) => update({ workingDir: event.target.value })}
              placeholder={t("Automatic")}
              spellCheck={false}
              className="h-10 flex-1 font-mono text-[11.5px]"
            />
            <div className="flex shrink-0 items-center gap-2">
              <Button variant="default" size="sm" className="h-10" icon={<FolderOpen className="size-3.5" />} onClick={() => void pickWorkingDir()}>
                {t("Browse")}
              </Button>
              {draft.workingDir.trim().length > 0 && (
                <Button variant="ghost" size="sm" className="h-10" onClick={() => update({ workingDir: "" })}>
                  {t("Clear")}
                </Button>
              )}
            </div>
          </div>
        </div>

        <div className="mt-6 flex flex-wrap items-center gap-3 border-t border-line pt-4">
          <Button variant="primary" size="sm" loading={saving} disabled={!dirty} onClick={() => void save()}>
            {t("Save Linux options")}
          </Button>
          {dirty && <span className="text-[11.5px] text-ink-3">{t("Unsaved changes")}</span>}
          {linux && (
            <span className="font-mono text-[10.5px] text-ink-3">
              {scan.data?.winetricks ? t("winetricks detected") : t("no winetricks")} ·{" "}
              {scan.data?.protonfixes ? t("protonfixes detected") : t("no protonfixes")}
            </span>
          )}
        </div>

        {linux && scan.data && scan.data.warnings.length > 0 && (
          <ul className="mt-3 flex flex-col gap-1 text-[11.5px] text-ink-3">
            {scan.data.warnings.map((warning) => (
              <li key={warning} className="border-l-2 border-line-2 pl-2">
                {warning}
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
