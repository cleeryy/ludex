import type { ReactNode } from "react";
import { motion } from "motion/react";
import { cn } from "@/lib/cn";
import { t } from "@/lib/i18n";
import { Kbd } from "@/components/ui/controls";
import { transitionOf } from "./motion";
import { SETTINGS_SECTIONS, type SettingsSectionId } from "./system-zones";

/* --------------------------------------------------------------------------
   Settings inside the shell (#196, #268).

   One axis: a horizontal category strip and one vertical stack of rows. A left
   column would make Left/Right both "change the value" and "change the
   column", which is the classic console trap. Every row is one focus stop; the
   value lives on the right, never as a separate dead stop.

   Cabinet: no eyebrow, no uppercase mono label, no second material branch. The
   title is 34u, a row label 28u, its detail 24u. Changes apply on the frame of
   the press — there is no Apply button anywhere in this surface.
   -------------------------------------------------------------------------- */

export const SETTINGS_SECTION_META: Record<
  SettingsSectionId,
  { label: () => string; description: () => string }
> = {
  language: {
    label: () => t("Language"),
    description: () => t("Applies to your account on every device. A device override stays on this device."),
  },
  appearance: {
    label: () => t("Appearance"),
    description: () => t("A room and eight knobs. Everything applies immediately and stays on this device."),
  },
  controllers: {
    label: () => t("Controllers"),
    description: () => t("Pad setup, vibration and the console cues on this device."),
  },
  audio: {
    label: () => t("Audio"),
    description: () => t("Output device, volume and mute for the console and every game without its own device."),
  },
  streaming: {
    label: () => t("Streaming"),
    description: () =>
      t("Publish the Ludex library as Sunshine apps and start streams from Moonlight. Ludex never bundles either application."),
  },
};

export type SettingsRowKind = "choice" | "toggle" | "action";

export interface SettingsRow {
  id: string;
  label: string;
  detail?: string;
  /** Current value, announced with the label by the row button. */
  value?: string;
  kind: SettingsRowKind;
  disabled?: boolean;
  reason?: string;
}

export function SettingsApp({
  section,
  rows,
  active,
  accountName,
  serverLabel,
  version,
  padLabel,
  preview,
  reduceMotion,
  instant,
  onSelectSection,
  onHighlight,
  onActivate,
}: {
  section: SettingsSectionId;
  rows: readonly SettingsRow[];
  active: number;
  accountName: string | null;
  serverLabel: string | null;
  version: string;
  padLabel: string;
  /** Live preview of the Appearance section: the room, rendered. */
  preview?: ReactNode;
  reduceMotion: boolean | null;
  /** A held shoulder switched sections: land on the final frame. */
  instant: boolean;
  onSelectSection: (section: SettingsSectionId) => void;
  onHighlight: (index: number) => void;
  onActivate: (index: number) => void;
}) {
  const meta = SETTINGS_SECTION_META[section];
  const current = rows[Math.min(Math.max(active, 0), Math.max(rows.length - 1, 0))];

  return (
    <section className="flex min-h-0 flex-1 flex-col overflow-hidden" aria-label={t("Settings")}>
      <div className="shrink-0 pt-4 bp-pad-x sm:pt-5">
        <div className="flex flex-wrap items-end justify-between gap-x-8 gap-y-3">
          <div className="min-w-0">
            <h1 className="text-bp-title font-semibold leading-[1.05] tracking-[-0.02em] text-ink">
              {t("Settings")}
            </h1>
            <p className="mt-1 max-w-[76ch] text-bp-meta leading-relaxed text-ink-2">
              {meta.description()}
            </p>
          </div>
        </div>

        <div role="group" aria-label={t("Settings")} className="mt-4 flex flex-wrap items-center gap-2">
          {SETTINGS_SECTIONS.map((id) => {
            const tab = SETTINGS_SECTION_META[id];
            const selected = id === section;
            return (
              <button
                key={id}
                type="button"
                tabIndex={-1}
                aria-current={selected ? "true" : undefined}
                data-bp-tab-active={selected ? "true" : undefined}
                onClick={() => onSelectSection(id)}
                className={cn(
                  "bp-chip inline-flex shrink-0 items-center",
                  selected
                    ? "border-line-2 bg-surface-2 text-ink"
                    : "border-transparent text-ink-3 hover:text-ink-2",
                )}
              >
                {tab.label()}
              </button>
            );
          })}
          <span className="ms-auto hidden items-center gap-2 text-bp-meta text-ink-3 md:flex">
            <Kbd>LB</Kbd>
            <Kbd>RB</Kbd>
            {t("Tabs")}
          </span>
        </div>

        {preview && (
          <div className="mt-3" data-bp-preview="true">
            {preview}
          </div>
        )}
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain py-1 bp-pad-x">
        <motion.div key={section} {...transitionOf("settings", { reduce: reduceMotion, instant })}>
          {rows.length === 0 ? (
            <p className="px-1 py-8 text-bp-body text-ink-3">{t("Unavailable")}</p>
          ) : (
            <ul>
              {rows.map((row, index) => {
                const selected = row === current;
                const disabled = row.disabled === true;
                return (
                  <li key={row.id}>
                    <button
                      type="button"
                      data-system-active={selected ? "true" : undefined}
                      data-gamepad-target="true"
                      data-bp-settings-row="true"
                      aria-current={selected ? "true" : undefined}
                      aria-disabled={disabled ? "true" : undefined}
                      aria-label={row.value ? `${row.label}: ${row.value}` : row.label}
                      onPointerEnter={() => onHighlight(index)}
                      onClick={() => onActivate(index)}
                      className={cn(
                        "bp-row relative mb-1 flex min-h-[calc(var(--bp-u)*64)] w-full items-center gap-4 px-3 py-2.5 text-start",
                        selected && "bg-surface-2",
                        disabled && "opacity-55",
                      )}
                    >
                      {selected && (
                        <span
                          aria-hidden="true"
                          className="pointer-events-none absolute inset-y-2 start-0 w-[calc(var(--bp-u)*3)] bg-signal"
                        />
                      )}
                      <span className="min-w-0 flex-1">
                        <span className="block truncate text-bp-body font-medium text-ink">{row.label}</span>
                        <span className="mt-1 block truncate text-bp-meta leading-relaxed text-ink-3">
                          {disabled && row.reason ? row.reason : row.detail}
                        </span>
                      </span>
                      <span className="flex shrink-0 items-center justify-end gap-2">
                        {row.kind === "choice" && (
                          <span className="bp-panel flex items-center gap-2 px-3 py-1 text-bp-meta text-ink-2">
                            <span className="text-ink-3" aria-hidden="true">‹</span>
                            <span className="min-w-[9ch] max-w-[20ch] truncate text-center">{row.value}</span>
                            <span className="text-ink-3" aria-hidden="true">›</span>
                          </span>
                        )}
                        {row.kind === "toggle" && (
                          <span
                            className={cn(
                              "inline-flex items-center gap-2 rounded-[var(--bp-radius-pill)] border px-3 py-1 text-bp-meta",
                              row.value === t("On")
                                ? "border-signal text-ink"
                                : "border-line text-ink-3",
                            )}
                          >
                            {row.value === t("On") && <span className="bp-dot" aria-hidden="true" />}
                            {row.value}
                          </span>
                        )}
                        {row.kind === "action" && (
                          <span className="text-bp-meta text-ink-3">{row.value ?? t("Change")}</span>
                        )}
                      </span>
                    </button>
                  </li>
                );
              })}
            </ul>
          )}
        </motion.div>
      </div>

      <dl className="grid shrink-0 grid-cols-2 gap-x-6 gap-y-2 border-t py-3 bp-chrome bp-pad-x md:grid-cols-4">
        {[
          { label: t("Version"), value: `v${version}`, numeric: true },
          { label: t("Server"), value: serverLabel ?? t("Unavailable"), numeric: false },
          { label: t("Account"), value: accountName ?? t("Unavailable"), numeric: false },
          { label: t("Controllers"), value: padLabel, numeric: false },
        ].map((fact) => (
          <div key={fact.label} className="min-w-0">
            <dt className="text-bp-meta text-ink-3">{fact.label}</dt>
            <dd className={cn("mt-0.5 truncate text-bp-meta text-ink-2", fact.numeric && "num")}>
              {fact.value}
            </dd>
          </div>
        ))}
      </dl>
    </section>
  );
}
