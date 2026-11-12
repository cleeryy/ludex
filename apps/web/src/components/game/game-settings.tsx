import { useEffect, useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { FileClock, FileCog, RotateCcw, Save, Search, SlidersHorizontal } from "lucide-react";
import { api } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { native } from "@/lib/native";
import { formatBytes, formatRelative } from "@/lib/format";
import { cn } from "@/lib/cn";
import {
  canLoadConfigFile,
  defaultConfigPath,
  filterConfigFiles,
  readOnlyReason,
} from "@/lib/config-files";
import {
  canKeepInSync,
  candidateLabel,
  confidenceTone,
  isResolvedField,
  mappingFor,
  targetForField,
} from "@/lib/config-fields";
import type {
  AdapterField,
  CommonFieldMatch,
  CommonFieldPreview,
  CommonFieldTarget,
  ConfigAdapter,
  ConfigBackupRecord,
  ConfigDocument,
  ConfigEditInput,
  ConfigFileInfo,
} from "@/lib/types";
import { Badge, Button, Input, Switch, Textarea } from "@/components/ui/controls";
import { Dialog } from "@/components/ui/dialog";
import { Select } from "@/components/ui/select";
import { useAuth } from "@/state/auth";
import { useToast } from "@/state/toasts";
import { SectionHeader } from "@/components/page";
import { t, useI18n } from "@/lib/i18n";

type Tab = "adapters" | "files" | "backups";

/**
 * Game settings tab (#105/#106). Every write goes through the Rust command that
 * backs the file up, validates types, records a diff and then replaces it
 * atomically. This component never edits a file itself.
 */
export function GameSettingsPanel({ gameId, title }: { gameId: number; title: string }) {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const [tab, setTab] = useState<Tab>("adapters");

  const listing = useQuery({
    queryKey: ["game-configs", gameId],
    queryFn: () => native.listGameConfigs(gameId),
  });
  const adapters = useQuery({
    queryKey: ["game-config-adapters", gameId],
    queryFn: () => native.configAdapters(gameId),
  });
  const backups = useQuery({
    queryKey: ["game-config-backups", gameId],
    queryFn: () => native.listConfigBackups(gameId),
  });

  const refresh = async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: ["game-configs", gameId] }),
      queryClient.invalidateQueries({ queryKey: ["game-config-adapters", gameId] }),
      queryClient.invalidateQueries({ queryKey: ["game-config-backups", gameId] }),
    ]);
  };

  const detected = (adapters.data?.adapters ?? []).filter((adapter) => adapter.detected);
  const tabs: Array<{ id: Tab; label: string; icon: React.ReactNode; count?: number }> = [
    { id: "adapters", label: t("Game settings"), icon: <SlidersHorizontal className="size-3.5" />, count: detected.length },
    { id: "files", label: t("Config files"), icon: <FileCog className="size-3.5" />, count: listing.data?.files.length },
    { id: "backups", label: t("Backups"), icon: <FileClock className="size-3.5" />, count: backups.data?.length },
  ];

  return (
    <div className="min-w-0">
      <SectionHeader
        title={t("Game settings")}
        description={t("Edit {title}'s own config files. Every change is backed up first and can be restored from the Backups tab.", { title })}
      />

      <div role="tablist" aria-label={t("Game settings")} className="mt-4 flex flex-wrap items-center gap-1 border-b border-line">
        {tabs.map((item) => (
          <button
            key={item.id}
            role="tab"
            type="button"
            aria-selected={tab === item.id}
            onClick={() => setTab(item.id)}
            className={cn(
              "-mb-px flex items-center gap-1.5 border-b px-3 py-2 text-[12.5px] transition-colors duration-100 motion-reduce:transition-none",
              tab === item.id
                ? "border-signal text-ink"
                : "border-transparent text-ink-3 hover:text-ink",
            )}
          >
            {item.icon}
            {item.label}
            {typeof item.count === "number" && item.count > 0 && (
              <span className="num text-[10.5px] text-ink-3">{item.count}</span>
            )}
          </button>
        ))}
      </div>

      <div className="pt-5">
        {tab === "adapters" && (
          <AdaptersTab
            gameId={gameId}
            adapters={detected}
            loading={adapters.isPending}
            error={adapters.isError ? errorMessage(adapters.error) : null}
            onSaved={() => void refresh()}
          />
        )}
        {tab === "files" && (
          <FilesTab
            gameId={gameId}
            files={listing.data?.files ?? []}
            loading={listing.isPending}
            error={listing.isError ? errorMessage(listing.error) : null}
            onSaved={() => void refresh()}
          />
        )}
        {tab === "backups" && (
          <BackupsTab
            gameId={gameId}
            records={backups.data ?? []}
            loading={backups.isPending}
            onRestored={() => {
              push({ title: t("Settings restored"), tone: "success" });
              void refresh();
            }}
          />
        )}
      </div>
    </div>
  );
}

/* ------------------------------- adapters -------------------------------- */

function AdaptersTab({
  gameId,
  adapters,
  loading,
  error,
  onSaved,
}: {
  gameId: number;
  adapters: ConfigAdapter[];
  loading: boolean;
  error: string | null;
  onSaved: () => void;
}) {
  if (loading) return <p className="text-[12.5px] text-ink-3">{t("Detecting the engine…")}</p>;
  if (error) return <p className="text-[12.5px] text-danger">{error}</p>;
  if (adapters.length === 0) {
    return (
      <div className="border border-dashed border-line px-4 py-6">
        <p className="text-[12.5px] text-ink-2">{t("No engine marker found in this install folder.")}</p>
        <p className="mt-1 text-[11.5px] leading-relaxed text-ink-3">
          {t("Use the Config files tab to edit an ini, cfg, toml or json file directly.")}
        </p>
      </div>
    );
  }
  return (
    <div className="flex flex-col gap-5">
      {adapters.map((adapter) => (
        <AdapterCard key={adapter.id} gameId={gameId} adapter={adapter} onSaved={onSaved} />
      ))}
    </div>
  );
}

function AdapterCard({
  gameId,
  adapter,
  onSaved,
}: {
  gameId: number;
  adapter: ConfigAdapter;
  onSaved: () => void;
}) {
  const { push } = useToast();
  const [draft, setDraft] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    const next: Record<string, string> = {};
    for (const field of adapter.fields) {
      next[field.id] = field.value ?? "";
    }
    setDraft(next);
  }, [adapter]);

  const changed = adapter.fields.filter((field) => {
    const value = draft[field.id] ?? "";
    return Boolean(field.file && field.key) && value !== (field.value ?? "");
  });

  const save = async () => {
    if (changed.length === 0) return;
    setBusy(true);
    try {
      // Group by file so one adapter save creates one backup per file.
      const byFile = new Map<string, ConfigEditInput[]>();
      for (const field of changed) {
        if (!field.file || !field.key) continue;
        const list = byFile.get(field.file) ?? [];
        list.push({ key: field.key, value: draft[field.id] ?? "" });
        byFile.set(field.file, list);
      }
      for (const [file, edits] of byFile) {
        await native.writeGameConfig(gameId, file, edits, "adapter");
      }
      push({
        title: t("Settings saved"),
        description: t("{count} change(s) backed up and written.", { count: changed.length }),
        tone: "success",
      });
      onSaved();
    } catch (cause) {
      push({ title: t("Could not save the settings"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="border border-line">
      <div className="flex flex-wrap items-baseline justify-between gap-3 border-b border-line px-4 py-3">
        <div className="flex items-baseline gap-2">
          <span className="text-[13px] font-medium text-ink">{adapter.label}</span>
          <Badge tone="outline">{adapter.evidence ?? t("detected")}</Badge>
        </div>
        <Button
          variant="default"
          size="sm"
          icon={<Save className="size-3.5" />}
          loading={busy}
          disabled={changed.length === 0}
          onClick={() => void save()}
        >
          {changed.length > 0 ? t("Save {count} change(s)", { count: changed.length }) : t("No changes")}
        </Button>
      </div>

      {adapter.fields.length > 0 ? (
        <div className="grid gap-4 px-4 py-4 sm:grid-cols-2">
          {adapter.fields.map((field) => (
            <AdapterControl
              key={field.id}
              field={field}
              value={draft[field.id] ?? ""}
              onChange={(value) => setDraft((current) => ({ ...current, [field.id]: value }))}
            />
          ))}
        </div>
      ) : (
        <p className="px-4 py-4 text-[12px] text-ink-3">
          {t("This install has no editable field in the detected format.")}
        </p>
      )}

      {adapter.notes.length > 0 && (
        <ul className="flex flex-col gap-1 border-t border-line px-4 py-3 text-[11px] leading-relaxed text-ink-3">
          {adapter.notes.map((note) => (
            <li key={note}>{note}</li>
          ))}
        </ul>
      )}
    </div>
  );
}

function AdapterControl({
  field,
  value,
  onChange,
}: {
  field: AdapterField;
  value: string;
  onChange: (value: string) => void;
}) {
  const disabled = !field.file || !field.key;
  const id = `adapter-${field.id}`;

  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-baseline justify-between gap-2">
        <label htmlFor={id} className="text-[12px] text-ink-2">
          {field.label}
        </label>
        {disabled && <span className="text-[10px] text-ink-3">{t("read-only")}</span>}
      </div>

      {field.control === "select" ? (
        <Select
          label={field.label}
          value={value}
          onChange={onChange}
          options={
            field.options.some((option) => option.value === value)
              ? field.options
              : [...field.options, { value, label: value }]
          }
          className="h-10 text-[12px]"
        />
      ) : field.control === "toggle" ? (
        <Switch
          checked={["true", "1", "yes", "on"].includes(value.toLowerCase())}
          onCheckedChange={(checked) => onChange(checked ? "true" : "false")}
          label={field.label}
          disabled={disabled}
        />
      ) : (
        <Input
          id={id}
          value={value}
          onChange={(event) => onChange(event.target.value)}
          inputMode={field.control === "number" ? "numeric" : "text"}
          type={field.control === "number" ? "number" : "text"}
          disabled={disabled}
          className="h-10 font-mono text-[12px]"
        />
      )}

      <p className="truncate font-mono text-[10px] text-ink-3">
        {field.file && field.key ? `${field.file} · ${field.key}` : field.note ?? t("Not stored in the install folder")}
      </p>
      {field.note && field.file && <p className="text-[10.5px] leading-relaxed text-ink-3">{field.note}</p>}
    </div>
  );
}

/* --------------------------------- files --------------------------------- */

function FilesTab({
  gameId,
  files,
  loading,
  error,
  onSaved,
}: {
  gameId: number;
  files: ConfigFileInfo[];
  loading: boolean;
  error: string | null;
  onSaved: () => void;
}) {
  const queryClient = useQueryClient();
  const [selected, setSelected] = useState<string>("");
  const [filter, setFilter] = useState("");

  useEffect(() => {
    if (!selected && files.length > 0) {
      setSelected(defaultConfigPath(files));
    }
  }, [files, selected]);

  const selectedFile = files.find((file) => file.path === selected);
  const document = useQuery({
    queryKey: ["game-config", gameId, selected],
    queryFn: () => native.readGameConfig(gameId, selected),
    enabled: canLoadConfigFile(selectedFile),
  });

  const visible = useMemo(() => filterConfigFiles(files, filter), [files, filter]);

  if (loading) return <p className="text-[12.5px] text-ink-3">{t("Looking for config files…")}</p>;
  if (error) return <p className="text-[12.5px] text-danger">{error}</p>;

  return (
    <>
      <CommonFieldsPanel gameId={gameId} onSaved={onSaved} />
      <div className="grid gap-6 lg:grid-cols-[280px_1fr]">
      <div>
        <div className="relative">
          <Search className="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-ink-3" aria-hidden="true" />
          <Input
            value={filter}
            onChange={(event) => setFilter(event.target.value)}
            placeholder={t("Filter files")}
            aria-label={t("Filter files")}
            className="h-9 pl-8 text-[12px]"
          />
        </div>
        <p className="mt-2 text-[10.5px] leading-relaxed text-ink-3">
          {t("Every text config file in the install folder is listed. Nothing is hidden by name.")}
        </p>
        <div className="mt-2 flex flex-col border-y border-line">
          {visible.length === 0 && (
            <p className="py-3 text-[11.5px] text-ink-3">{t("No config file found in this install folder.")}</p>
          )}
          {visible.map((file) => {
            const reason = readOnlyReason(file);
            return (
              <button
                key={file.path}
                type="button"
                onClick={() => setSelected(file.path)}
                className={cn(
                  "flex flex-col gap-0.5 border-b border-line px-2 py-2 text-left transition-colors duration-100 last:border-0 hover:bg-surface motion-reduce:transition-none",
                  selected === file.path && "bg-surface",
                )}
              >
                <span className="flex items-baseline justify-between gap-2">
                  <span className="truncate text-[12px] text-ink">{file.name}</span>
                  <span className="shrink-0 font-mono text-[9.5px] uppercase text-ink-3">{file.format}</span>
                </span>
                <span className="truncate font-mono text-[10px] text-ink-3">
                  {file.path} · {formatBytes(file.sizeBytes)} · {t("{count} keys", { count: file.entries })}
                </span>
                {!file.editable && (
                  <span className="text-[10px] text-danger">{reason ?? t("Read-only")}</span>
                )}
              </button>
            );
          })}
        </div>
      </div>

      <div className="min-w-0">
        {selectedFile && !selectedFile.editable && (
          <div className="flex flex-col gap-1.5">
            <Badge tone="outline">{t("Read-only")}</Badge>
            <p className="text-[12.5px] leading-relaxed text-ink-3">
              {readOnlyReason(selectedFile) ?? t("Read-only")}
            </p>
            <p className="font-mono text-[10.5px] text-ink-3">
              {selectedFile.path} · {formatBytes(selectedFile.sizeBytes)}
            </p>
          </div>
        )}
        {document.isError && selectedFile?.editable && (
          <p className="text-[12.5px] text-danger">{errorMessage(document.error)}</p>
        )}
        {document.isPending && selectedFile?.editable && (
          <p className="text-[12.5px] text-ink-3">{t("Opening…")}</p>
        )}
        {document.data && selectedFile?.editable && (
          <KeyEditor
            gameId={gameId}
            document={document.data}
            onSaved={() => {
              void queryClient.invalidateQueries({ queryKey: ["game-config", gameId, selected] });
              onSaved();
            }}
          />
        )}
        {!selected && files.length > 0 && (
          <p className="text-[12.5px] text-ink-3">{t("Choose a file on the left.")}</p>
        )}
      </div>
      </div>
    </>
  );
}

function KeyEditor({
  gameId,
  document,
  onSaved,
}: {
  gameId: number;
  document: ConfigDocument;
  onSaved: () => void;
}) {
  const { push } = useToast();
  const [draft, setDraft] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    const next: Record<string, string> = {};
    for (const entry of document.entries) next[entry.key] = entry.value;
    setDraft(next);
  }, [document]);

  const changed = document.entries.filter((entry) => (draft[entry.key] ?? entry.value) !== entry.value);

  const save = async () => {
    if (changed.length === 0) return;
    setBusy(true);
    try {
      await native.writeGameConfig(
        gameId,
        document.path,
        changed.map((entry) => ({ key: entry.key, value: draft[entry.key] ?? entry.value })),
        "manual",
      );
      push({
        title: t("Config saved"),
        description: t("The previous version is in the Backups tab."),
        tone: "success",
      });
      onSaved();
    } catch (cause) {
      push({ title: t("Could not save the config"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setBusy(false);
    }
  };

  return (
    <div>
      <div className="flex flex-wrap items-baseline justify-between gap-3">
        <div className="min-w-0">
          <p className="truncate text-[13px] text-ink">{document.path}</p>
          <p className="mt-0.5 font-mono text-[10.5px] text-ink-3">
            {document.format} · {formatBytes(document.sizeBytes)} · {t("{count} keys", { count: document.entries.length })}
          </p>
        </div>
        <Button
          variant="primary"
          size="sm"
          icon={<Save className="size-3.5" />}
          loading={busy}
          disabled={changed.length === 0}
          onClick={() => void save()}
        >
          {changed.length > 0 ? t("Save {count} change(s)", { count: changed.length }) : t("No changes")}
        </Button>
      </div>

      <div className="mt-3 max-h-[420px] overflow-y-auto border-y border-line">
        {document.note && (
          <p className="border-b border-line px-2 py-2 text-[11px] leading-relaxed text-ink-3">
            {document.note}
          </p>
        )}
        {document.entries.length === 0 && (
          <p className="py-3 text-[12px] text-ink-3">{t("This file has no editable scalar key.")}</p>
        )}
        {document.entries.map((entry) => (
          <div key={entry.key} className="grid gap-2 border-b border-line px-2 py-2 last:border-0 sm:grid-cols-[1fr_180px_64px] sm:items-center">
            <div className="min-w-0">
              <p className="truncate font-mono text-[11px] text-ink-2">{entry.key}</p>
              {entry.line > 0 && <p className="font-mono text-[9.5px] text-ink-3">{t("line {line}", { line: entry.line })}</p>}
            </div>
            <Input
              value={draft[entry.key] ?? entry.value}
              onChange={(event) => setDraft((current) => ({ ...current, [entry.key]: event.target.value }))}
              aria-label={entry.key}
              className={cn(
                "h-8 font-mono text-[11px]",
                (draft[entry.key] ?? entry.value) !== entry.value && "border-signal",
              )}
            />
            <span className="font-mono text-[9.5px] uppercase text-ink-3">{entry.valueType}</span>
          </div>
        ))}
      </div>
      <p className="mt-2 text-[10.5px] leading-relaxed text-ink-3">
        {t("Comments and ordering are preserved. Ludex refuses binary files, files over 1 MiB and values that do not match the existing type.")}
      </p>
      <p className="mt-1 text-[10.5px] leading-relaxed text-ink-3">
        {t("The file keeps its encoding (UTF-8, UTF-16 or Windows-1252) and line endings when saved.")}
      </p>
    </div>
  );
}

/* ------------------------------ common fields ---------------------------- */

type PendingField = {
  match: CommonFieldMatch;
  target: CommonFieldTarget;
  preview: CommonFieldPreview;
};

function confidenceLabel(confidence: CommonFieldMatch["confidence"]): string {
  if (confidence === "certain") return t("Certain");
  if (confidence === "likely") return t("Likely");
  return t("Ambiguous");
}

function fieldLabel(field: CommonFieldMatch["field"]): string {
  return field === "language" ? t("Language") : t("Player name");
}

/**
 * "Common fields" panel (#148). Detection is read-only; apply is always an
 * explicit preview → confirm, and an ambiguous match only offers a pin. The
 * raw key list below stays the fallback for everything else.
 */
function CommonFieldsPanel({ gameId, onSaved }: { gameId: number; onSaved: () => void }) {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const { locale } = useI18n();
  const { user } = useAuth();
  const profile = useQuery({
    queryKey: ["my-profile"],
    queryFn: () => api.myProfile(),
    enabled: Boolean(user),
    staleTime: 60_000,
    retry: false,
  });
  const displayName = profile.data?.displayName?.trim() || user?.username || "";

  const report = useQuery({
    queryKey: ["game-config-common-fields", gameId],
    queryFn: () => native.commonConfigFields(gameId),
  });
  const sync = useQuery({
    queryKey: ["game-config-sync", gameId],
    queryFn: () => native.configSyncPrefs(gameId),
  });
  const [pending, setPending] = useState<PendingField | null>(null);
  const [applying, setApplying] = useState(false);
  const [toggling, setToggling] = useState(false);
  const [template, setTemplate] = useState<string | null>(null);

  const refresh = () => {
    void queryClient.invalidateQueries({ queryKey: ["game-config-common-fields", gameId] });
    void queryClient.invalidateQueries({ queryKey: ["game-config-sync", gameId] });
    onSaved();
  };

  const openPreview = async (match: CommonFieldMatch, target: CommonFieldTarget) => {
    if (!match.file || !match.key) return;
    try {
      const preview = await native.previewCommonField(gameId, match.file, match.key, target);
      setPending({ match, target, preview });
    } catch (cause) {
      push({ title: t("Could not preview this change"), description: errorMessage(cause), tone: "danger" });
    }
  };

  const applyPending = async () => {
    if (!pending?.match.file || !pending.match.key) return;
    setApplying(true);
    try {
      await native.applyCommonField(gameId, pending.match.file, pending.match.key, pending.target);
      push({
        title: t("Config field applied"),
        description: `${pending.match.file} · ${pending.match.key}`,
        tone: "success",
      });
      setPending(null);
      refresh();
    } catch (cause) {
      push({ title: t("Could not apply this field"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setApplying(false);
    }
  };

  const pin = async (field: CommonFieldMatch["field"], file: string, key: string) => {
    try {
      await native.saveConfigFieldMapping(gameId, field, file, key);
      push({ title: t("Key pinned"), description: `${file} · ${key}`, tone: "success" });
      refresh();
    } catch (cause) {
      push({ title: t("Could not pin this key"), description: errorMessage(cause), tone: "danger" });
    }
  };

  const unpin = async (field: CommonFieldMatch["field"]) => {
    try {
      await native.deleteConfigFieldMapping(gameId, field);
      push({ title: t("Detection restored"), tone: "success" });
      refresh();
    } catch (cause) {
      push({ title: t("Could not unpin this key"), description: errorMessage(cause), tone: "danger" });
    }
  };

  const toggleSync = async (enabled: boolean) => {
    setToggling(true);
    try {
      await native.setConfigSyncPrefs(gameId, { enabled, language: locale, playerName: displayName });
      push({ title: t("Sync preference saved"), tone: "success" });
      refresh();
    } catch (cause) {
      push({ title: t("Could not save the sync preference"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setToggling(false);
    }
  };

  const exportTemplate = async () => {
    try {
      setTemplate(await native.exportConfigMappings(gameId));
    } catch (cause) {
      push({ title: t("Could not export the template"), description: errorMessage(cause), tone: "danger" });
    }
  };

  const copyTemplate = async () => {
    if (!template) return;
    try {
      await navigator.clipboard.writeText(template);
      push({ title: t("Copied"), tone: "success" });
    } catch {
      push({ title: t("Select the text and copy it manually."), tone: "danger" });
    }
  };

  if (report.isPending || report.isError || !report.data) return null;
  const fields = report.data.fields;
  if (fields.length === 0) return null;
  const syncOn = sync.data?.enabled ?? false;
  const languageTarget = targetForField("language", locale, displayName);
  const nameTarget = targetForField("playerName", locale, displayName);

  return (
    <section aria-labelledby="common-fields-heading" className="mb-6 border border-line">
      <div className="flex flex-wrap items-start justify-between gap-3 border-b border-line px-4 py-3">
        <div className="min-w-0">
          <h3 id="common-fields-heading" className="text-[13px] font-medium text-ink">
            {t("Common fields")}
          </h3>
          <p className="mt-1 max-w-[70ch] text-[11px] leading-relaxed text-ink-3">
            {t("Ludex recognised the language and player name this game stores. Applying one previews the change and backs the file up first.")}
          </p>
        </div>
        {report.data.mappings.length > 0 && (
          <Button variant="ghost" size="sm" onClick={() => void exportTemplate()}>
            {t("Export template")}
          </Button>
        )}
      </div>

      <div className="flex flex-wrap items-center justify-between gap-3 border-b border-line px-4 py-3">
        <div className="min-w-0">
          <p className="text-[12px] text-ink-2">{t("Keep in sync on launch")}</p>
          <p className="mt-0.5 max-w-[70ch] text-[11px] leading-relaxed text-ink-3">
            {t(
              "Ludex applies {language} and {name} without asking the next time this game starts.",
              { language: locale.toUpperCase(), name: displayName || t("your profile name") },
            )}
          </p>
        </div>
        <Switch
          checked={syncOn}
          disabled={sync.isPending || toggling || !canKeepInSync(report.data)}
          onCheckedChange={(checked) => void toggleSync(checked)}
          label={t("Keep in sync on launch")}
        />
      </div>

      {fields.map((match) => {
        const pinned = match.source === "template" ? mappingFor(report.data, match.field) : null;
        return (
          <div key={match.field} className="border-b border-line px-4 py-3 last:border-0">
            <div className="flex flex-wrap items-baseline gap-2">
              <span className="text-[12.5px] font-medium text-ink">{fieldLabel(match.field)}</span>
              <Badge tone={confidenceTone(match.confidence)}>{confidenceLabel(match.confidence)}</Badge>
              <Badge tone="outline">{match.source === "template" ? t("Game template") : t("Built-in")}</Badge>
              {isResolvedField(match) && (
                <span className="min-w-0 truncate font-mono text-[10.5px] text-ink-3">
                  {match.file} · {match.key}
                </span>
              )}
            </div>

            {isResolvedField(match) ? (
              <>
                <p className="mt-1 text-[11.5px] text-ink-2">
                  {t("Current value")}:{" "}
                  <span className="font-mono text-ink">{match.value || t("empty")}</span>
                </p>
                {match.allowedValues.length > 0 && (
                  <p className="mt-0.5 text-[10.5px] text-ink-3">
                    {t("Accepted values: {values}", { values: match.allowedValues.join(", ") })}
                  </p>
                )}
                {match.note && <p className="mt-0.5 text-[10.5px] leading-relaxed text-ink-3">{match.note}</p>}

                <div className="mt-2 flex flex-wrap items-center gap-2">
                  {match.field === "language" ? (
                    languageTarget ? (
                      <Button
                        variant="default"
                        size="sm"
                        onClick={() => void openPreview(match, languageTarget)}
                      >
                        {t("Apply Ludex language ({code})", { code: languageTarget.value })}
                      </Button>
                    ) : null
                  ) : nameTarget ? (
                    <Button variant="default" size="sm" onClick={() => void openPreview(match, nameTarget)}>
                      {t("Apply my name ({name})", { name: nameTarget.value })}
                    </Button>
                  ) : (
                    <span className="text-[10.5px] text-ink-3">
                      {t("Set a display name in Settings → Profile to apply it here.")}
                    </span>
                  )}
                  {pinned ? (
                    <Button variant="ghost" size="sm" onClick={() => void unpin(match.field)}>
                      {t("Use detection again")}
                    </Button>
                  ) : (
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => void pin(match.field, match.file, match.key)}
                    >
                      {t("Pin this key")}
                    </Button>
                  )}
                </div>
              </>
            ) : (
              <>
                <p className="mt-1 text-[11.5px] leading-relaxed text-ink-2">
                  {t("Several keys match. Choose the one this game reads; Ludex will not guess.")}
                </p>
                <div className="mt-2 flex flex-col divide-y divide-line border-y border-line">
                  {match.candidates.map((candidate) => (
                    <div
                      key={candidateLabel(candidate)}
                      className="flex flex-wrap items-center justify-between gap-2 py-2"
                    >
                      <span className="min-w-0 truncate font-mono text-[10.5px] text-ink-3">
                        {candidateLabel(candidate)} ={" "}
                        <span className="text-ink-2">{candidate.value || t("empty")}</span>
                      </span>
                      <Button
                        variant="ghost"
                        size="sm"
                        onClick={() => void pin(match.field, candidate.file, candidate.key)}
                      >
                        {t("Use this key")}
                      </Button>
                    </div>
                  ))}
                </div>
              </>
            )}
          </div>
        );
      })}

      <Dialog
        open={Boolean(pending)}
        onOpenChange={(open) => {
          if (!open && !applying) setPending(null);
        }}
        title={pending ? t("Apply {field}?", { field: fieldLabel(pending.match.field) }) : ""}
        description={pending ? `${pending.match.file ?? ""} · ${pending.match.key ?? ""}` : undefined}
        size="sm"
        footer={
          <>
            <Button variant="ghost" onClick={() => setPending(null)} disabled={applying}>
              {t("Cancel")}
            </Button>
            <Button
              variant="primary"
              loading={applying}
              disabled={pending?.preview.unchanged ?? true}
              onClick={() => void applyPending()}
            >
              {t("Apply")}
            </Button>
          </>
        }
      >
        {pending && (
          <div>
            <div className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 font-mono text-[11px]">
              <span className="text-ink-3">{t("Before")}</span>
              <span className="break-all text-ink-2">{pending.preview.currentValue || t("empty")}</span>
              <span className="text-ink-3">{t("After")}</span>
              <span className="break-all text-ink">{pending.preview.nextValue || t("empty")}</span>
            </div>
            {pending.preview.unchanged ? (
              <p className="mt-3 text-[11.5px] text-ink-2">
                {t("This value is already set; nothing will be written.")}
              </p>
            ) : (
              <pre className="mt-3 max-h-40 overflow-auto whitespace-pre-wrap border-l-2 border-line-2 pl-3 font-mono text-[10px] leading-relaxed text-ink-3">
                {pending.preview.diff}
              </pre>
            )}
            <p className="mt-2 text-[10.5px] leading-relaxed text-ink-3">
              {t("The file is copied to the Backups tab before it changes, and keeps its encoding.")}
            </p>
          </div>
        )}
      </Dialog>

      <Dialog
        open={template !== null}
        onOpenChange={(open) => !open && setTemplate(null)}
        title={t("Game template")}
        description={t("Pinned keys exported as JSON.")}
        size="sm"
        footer={
          <>
            <Button variant="ghost" onClick={() => setTemplate(null)}>
              {t("Close")}
            </Button>
            <Button variant="primary" onClick={() => void copyTemplate()}>
              {t("Copy")}
            </Button>
          </>
        }
      >
        <Textarea
          readOnly
          rows={10}
          value={template ?? ""}
          aria-label={t("Game template")}
          className="font-mono text-[10.5px]"
        />
      </Dialog>
    </section>
  );
}

/* -------------------------------- backups -------------------------------- */

function BackupsTab({
  gameId,
  records,
  loading,
  onRestored,
}: {
  gameId: number;
  records: ConfigBackupRecord[];
  loading: boolean;
  onRestored: () => void;
}) {
  const { push } = useToast();
  const [confirm, setConfirm] = useState<ConfigBackupRecord | null>(null);
  const [busy, setBusy] = useState(false);

  if (loading) return <p className="text-[12.5px] text-ink-3">{t("Reading the backup history…")}</p>;

  const restore = async (record: ConfigBackupRecord) => {
    setBusy(true);
    try {
      await native.restoreGameConfig(gameId, record.id);
      setConfirm(null);
      onRestored();
    } catch (cause) {
      push({ title: t("Could not restore this backup"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setBusy(false);
    }
  };

  return (
    <div>
      {records.length === 0 && (
        <p className="border border-dashed border-line px-4 py-5 text-[12px] text-ink-3">
          {t("No backup yet. The first config change creates one automatically.")}
        </p>
      )}
      <div className="flex flex-col divide-y divide-line border-y border-line">
        {records.map((record) => (
          <div key={record.id} className="py-3">
            <div className="flex flex-wrap items-baseline justify-between gap-2">
              <div className="min-w-0">
                <p className="truncate text-[12.5px] text-ink">{record.filePath}</p>
                <p className="mt-0.5 font-mono text-[10px] text-ink-3">
                  {formatRelative(record.createdAt)} · {formatBytes(record.bytes)} · {record.origin}
                </p>
              </div>
              <Button
                variant="ghost"
                size="sm"
                icon={<RotateCcw className="size-3.5" />}
                onClick={() => setConfirm(record)}
              >
                {t("Restore")}
              </Button>
            </div>
            {record.diff && (
              <pre className="mt-2 max-h-32 overflow-auto whitespace-pre-wrap border-l-2 border-line-2 pl-3 font-mono text-[10px] leading-relaxed text-ink-3">
                {record.diff}
              </pre>
            )}
          </div>
        ))}
      </div>

      <Dialog
        open={Boolean(confirm)}
        onOpenChange={(open) => !open && setConfirm(null)}
        title={t("Restore this backup?")}
        description={confirm?.filePath}
        size="sm"
        footer={
          <>
            <Button variant="ghost" onClick={() => setConfirm(null)}>
              {t("Cancel")}
            </Button>
            <Button variant="primary" loading={busy} onClick={() => confirm && void restore(confirm)}>
              {t("Restore")}
            </Button>
          </>
        }
      >
        <p className="text-[12.5px] leading-relaxed text-ink-2">
          {t("The current file is copied to a new backup before the restore, so this too can be undone.")}
        </p>
      </Dialog>
    </div>
  );
}
