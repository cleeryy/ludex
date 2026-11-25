import { useState } from "react";
import { BookmarkPlus, Dices, X } from "lucide-react";
import { cn } from "@/lib/cn";
import {
  addCustomPreset,
  builtinPresets,
  deleteCustomPreset,
  extractPresetParams,
  loadCustomPresets,
  matchesPreset,
  type LibraryPreset,
} from "@/lib/library-presets";
import { Button, Input } from "@/components/ui/controls";
import { Dialog } from "@/components/ui/dialog";
import { t } from "@/lib/i18n";

/**
 * Horizontal preset strip for the Library screen (#348): one-click built-in
 * slices plus user-saved filter views persisted in server-scoped localStorage.
 */
export function PresetsBar({
  params,
  includeInstalled,
  hasFilters,
  onApply,
  onClear,
  inline = false,
}: {
  params: URLSearchParams;
  includeInstalled: boolean;
  hasFilters: boolean;
  onApply: (preset: Readonly<Record<string, string>>) => void;
  onClear: () => void;
  /** Compact inline mode for embedding inside the unified library pill strip. */
  inline?: boolean;
}) {
  const [custom, setCustom] = useState<LibraryPreset[]>(() => loadCustomPresets());
  const [saveOpen, setSaveOpen] = useState(false);
  const [name, setName] = useState("");

  const presets = [...builtinPresets(includeInstalled), ...custom];
  const anyMatched = presets.some((preset) => matchesPreset(params, preset.params));

  const saveCurrent = () => {
    const trimmed = name.trim();
    if (!trimmed) return;
    const next = addCustomPreset(trimmed, extractPresetParams(params));
    setCustom(next);
    setName("");
    setSaveOpen(false);
  };

  const removeCustom = (id: string) => {
    setCustom(deleteCustomPreset(id));
  };

  return (
    <div
      className={cn(
        "flex items-center gap-1.5",
        inline ? "shrink-0 flex-nowrap" : "mt-2.5 flex-wrap",
      )}
      aria-label={t("Saved views")}
    >
      {!inline && <span className="mr-1 text-micro uppercase tracking-[0.12em] text-ink-3">{t("Views")}</span>}
      {presets.map((preset) => {
        const active = matchesPreset(params, preset.params);
        return (
          <span
            key={preset.id}
            className={cn(
              "inline-flex h-7 shrink-0 items-center rounded-[var(--r-sm)] border transition-colors",
              active
                ? "border-signal/60 bg-signal/12 text-signal-text"
                : "border-line-2 text-ink-3 hover:border-ink-3 hover:text-ink-2",
            )}
          >
            <button
              type="button"
              onClick={() => (active ? onClear() : onApply(preset.params))}
              aria-pressed={active}
              className="inline-flex h-full items-center whitespace-nowrap px-2 text-meta"
            >
              {preset.name}
            </button>
            {!preset.builtin && (
              <button
                type="button"
                onClick={() => removeCustom(preset.id)}
                aria-label={t("Remove preset {name}", { name: preset.name })}
                className="inline-flex h-full items-center border-l border-line px-1.5 text-ink-3 transition-colors hover:text-ink"
              >
                <X className="size-3" aria-hidden="true" />
              </button>
            )}
          </span>
        );
      })}

      {hasFilters && !anyMatched && (
        <button
          type="button"
          onClick={() => setSaveOpen(true)}
          className="inline-flex h-7 shrink-0 items-center gap-1 whitespace-nowrap rounded-[var(--r-sm)] border border-dashed border-line-2 px-2 text-meta text-ink-3 transition-colors hover:border-ink-3 hover:text-ink"
        >
          <BookmarkPlus className="size-3.5" aria-hidden="true" />
          {t("Save view")}
        </button>
      )}

      <Dialog
        open={saveOpen}
        onOpenChange={setSaveOpen}
        title={t("Save filter view")}
        description={t("Keep this filter and sort combination as a one-click preset on this device.")}
        footer={
          <>
            <Button variant="ghost" size="sm" onClick={() => setSaveOpen(false)}>
              {t("Cancel")}
            </Button>
            <Button variant="primary" size="sm" disabled={!name.trim()} onClick={saveCurrent}>
              {t("Save preset")}
            </Button>
          </>
        }
      >
        <form
          onSubmit={(event) => {
            event.preventDefault();
            saveCurrent();
          }}
        >
          <label htmlFor="preset-name" className="block text-label text-ink-2">
            {t("Preset name")}
          </label>
          <Input
            id="preset-name"
            value={name}
            maxLength={40}
            autoFocus
            placeholder={t("Co-op weekend")}
            onChange={(event) => setName(event.target.value)}
            className="mt-1.5"
          />
        </form>
      </Dialog>
    </div>
  );
}

/** Random backlog picker button (#348). */
export function SurpriseMeButton({
  loading,
  disabled,
  onClick,
}: {
  loading: boolean;
  disabled?: boolean;
  onClick: () => void;
}) {
  return (
    <Button
      variant="ghost"
      size="sm"
      loading={loading}
      disabled={disabled}
      icon={<Dices className="size-3.5" />}
      onClick={onClick}
    >
      {t("Surprise me")}
    </Button>
  );
}
