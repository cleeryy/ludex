import { useState } from "react";
import { cn } from "@/lib/cn";
import {
  coverageFor,
  localePickerEntries,
  localeSourceLabel,
  statusLabel,
  t,
  useI18n,
  type LocaleMeta,
} from "@/lib/i18n";
import { errorMessage } from "@/lib/http";
import { useAuth } from "@/state/auth";
import { SectionHeader } from "@/components/page";
import { Badge, Button, Switch } from "@/components/ui/controls";

/**
 * Language picker (Settings → Language, #139 and #188).
 *
 * The effective language is resolved device override → account preference →
 * server default → browser → English. The panel always states the effective
 * value and where it comes from, lists every registered locale (below the
 * 95 % coverage gate they stay visible but disabled with the reason), and saves
 * to the account by default. "Only this device" is the secondary path and
 * writes the local override instead.
 *
 * Changing a language applies immediately: `setLocale`/`updateLocale` load the
 * catalog and remount the tree; no reload and no flash.
 */
export function LanguagePanel() {
  const {
    locale: activeLocale,
    source,
    deviceOverride,
    setLocale,
    clearDeviceOverride,
  } = useI18n();
  const { user, updateLocale } = useAuth();
  const [deviceOnly, setDeviceOnly] = useState(false);
  const [saving, setSaving] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const entries = localePickerEntries(activeLocale);
  const official = entries.filter((entry) => entry.meta.status === "official");
  const community = entries.filter((entry) => entry.meta.status !== "official");
  const groups = [
    { key: "official", title: t("Official languages"), entries: official },
    { key: "community", title: t("Community translations"), entries: community },
  ].filter((group) => group.entries.length > 0);

  const choose = async (entry: LocaleMeta) => {
    setError(null);
    setSaving(entry.code);
    try {
      // A signed-out visitor has no account to save to; the only honest target
      // is this device.
      if (!user || deviceOnly) await setLocale(entry.code);
      else await updateLocale(entry.code);
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setSaving(null);
    }
  };

  const useAccountDefault = async () => {
    setError(null);
    try {
      await clearDeviceOverride();
    } catch (cause) {
      setError(errorMessage(cause));
    }
  };

  const useServerDefault = async () => {
    setError(null);
    try {
      // The server default only applies when neither a device override nor an
      // account preference outranks it, so this clears both.
      if (user) await updateLocale(null);
      else await clearDeviceOverride();
    } catch (cause) {
      setError(errorMessage(cause));
    }
  };

  return (
    <div>
      <SectionHeader
        title={t("Language")}
        description={t("Applies to your account on every device. A device override stays on this device.")}
      />

      <div className="flex flex-wrap items-center justify-between gap-3 border border-line bg-surface-2 px-3 py-2.5">
        <div className="min-w-0">
          <p className="font-mono text-[10px] uppercase tracking-[0.12em] text-ink-3">{t("Effective language")}</p>
          <p className="mt-0.5 truncate text-[13px] text-ink">
            <span lang={activeLocale}>{entries.find((entry) => entry.selected)?.meta.label ?? activeLocale}</span>
          </p>
        </div>
        <Badge tone={source === "device" ? "signal" : "outline"}>{localeSourceLabel(source)}</Badge>
      </div>

      {deviceOverride && (
        <div className="mt-2 flex flex-wrap items-center justify-end gap-2">
          {user && (
            <Button variant="default" size="sm" onClick={() => void useAccountDefault()}>
              {t("Use account default")}
            </Button>
          )}
          <Button variant="ghost" size="sm" onClick={() => void useServerDefault()}>
            {t("Use server default")}
          </Button>
        </div>
      )}

      <div className="mt-4 flex items-center justify-between gap-3">
        <p className="text-[11.5px] leading-relaxed text-ink-3">{t("Changes apply immediately.")}</p>
        {user && (
          <div className="flex shrink-0 items-center gap-2 text-[11.5px] text-ink-2">
            <Switch
              checked={deviceOnly}
              onCheckedChange={setDeviceOnly}
              label={t("Only this device")}
            />
            {t("Only this device")}
          </div>
        )}
      </div>

      {groups.map((group) => (
        <div key={group.key} className="mt-4 first:mt-0">
          <p className="mb-2 font-mono text-[10px] uppercase tracking-[0.12em] text-ink-3">{group.title}</p>
          <div className="grid gap-1.5 sm:grid-cols-2">
            {group.entries.map((entry) => (
              <LocaleButton
                key={entry.meta.code}
                entry={entry.meta}
                selected={entry.selected}
                disabled={entry.disabled}
                reason={entry.reason}
                saving={saving === entry.meta.code}
                onSelect={() => void choose(entry.meta)}
              />
            ))}
          </div>
        </div>
      ))}

      {error && (
        <p role="alert" className="mt-3 text-[11.5px] leading-relaxed text-danger">
          {t("Could not save the language")}: {error}
        </p>
      )}
      <p className="mt-3 text-[11.5px] leading-relaxed text-ink-3">
        {t("Community locales are contributed as JSON files. See docs/I18N.md for the contribution flow; Arabic is machine-seeded and unreviewed.")}
      </p>
    </div>
  );
}

function LocaleButton({
  entry,
  selected,
  disabled,
  reason,
  saving,
  onSelect,
}: {
  entry: LocaleMeta;
  selected: boolean;
  disabled: boolean;
  reason: string | null;
  saving: boolean;
  onSelect: () => void;
}) {
  const coverage = coverageFor(entry.code);
  return (
    <button
      type="button"
      lang={entry.code}
      dir={entry.dir}
      aria-pressed={selected}
      aria-disabled={disabled}
      disabled={disabled || saving}
      title={reason ?? undefined}
      onClick={onSelect}
      className={cn(
        "flex min-h-11 items-center justify-between gap-3 border px-3 py-2 text-start transition-colors",
        selected ? "border-signal bg-surface-2" : "border-line hover:border-line-2 hover:bg-surface",
        disabled && "cursor-not-allowed opacity-55 hover:border-line hover:bg-transparent",
      )}
    >
      <span className="min-w-0">
        <span className="block truncate text-[13px] text-ink">{entry.label}</span>
        <span className="block truncate font-mono text-[10px] text-ink-3">
          {entry.englishLabel}
          {entry.dir === "rtl" ? " · RTL" : ""}
        </span>
        {reason && <span className="mt-0.5 block text-[10.5px] leading-snug text-ink-3">{reason}</span>}
      </span>
      <span className="flex shrink-0 items-center gap-1.5">
        {entry.code !== "en" && coverage && <span className="num text-[10px] text-ink-3">{coverage.percent}%</span>}
        <Badge tone={selected ? "signal" : "outline"}>{statusLabel(entry)}</Badge>
      </span>
    </button>
  );
}
