import type { ComponentType } from "react";
import {
  Cast,
  ChevronLeft,
  ChevronRight,
  CornerDownLeft,
  Gamepad2,
  Globe2,
  Palette,
} from "lucide-react";
import { cn } from "@/lib/cn";
import { t } from "@/lib/i18n";
import { UserAvatar } from "@/components/community/user-avatar";
import { Kbd } from "@/components/ui/controls";
import { SETTINGS_SECTIONS, type SettingsSectionId } from "./system-zones";

/* --------------------------------------------------------------------------
   Settings inside the shell (#196).

   One axis: a horizontal category strip and one vertical stack of rows. A
   left column would make Left/Right both "change the value" and "change the
   column", which is the classic console trap. Every row is one focus stop;
   readouts live in the row's second line, never as a separate dead stop.
   -------------------------------------------------------------------------- */

type Icon = ComponentType<{ className?: string; "aria-hidden"?: boolean | "true" | "false" }>;

export const SETTINGS_SECTION_META: Record<
  SettingsSectionId,
  { icon: Icon; label: () => string; description: () => string }
> = {
  language: {
    icon: Globe2,
    label: () => t("Language"),
    description: () => t("Applies to your account on every device. A device override stays on this device."),
  },
  appearance: {
    icon: Palette,
    label: () => t("Appearance"),
    description: () => t("Themes, accent colour and the shape of the window on this device."),
  },
  controllers: {
    icon: Gamepad2,
    label: () => t("Controllers"),
    description: () => t("Pad setup, vibration and the console cues on this device."),
  },
  streaming: {
    icon: Cast,
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
  accountAvatar,
  serverLabel,
  version,
  padLabel,
  onSelectSection,
  onHighlight,
  onActivate,
}: {
  section: SettingsSectionId;
  rows: readonly SettingsRow[];
  active: number;
  accountName: string | null;
  accountAvatar: string | null;
  serverLabel: string | null;
  version: string;
  padLabel: string;
  onSelectSection: (section: SettingsSectionId) => void;
  onHighlight: (index: number) => void;
  onActivate: (index: number) => void;
}) {
  const meta = SETTINGS_SECTION_META[section];
  const current = rows[Math.min(Math.max(active, 0), Math.max(rows.length - 1, 0))];

  return (
    <section className="flex min-h-0 flex-1 flex-col overflow-hidden" aria-label={t("Settings")}>
      <div className="shrink-0 pb-0 pt-4 bp-pad-x sm:pt-5">
        <div className="flex flex-wrap items-start justify-between gap-x-8 gap-y-2">
          <div className="min-w-0">
            <p className="font-mono text-[9.5px] uppercase tracking-[0.18em] text-signal">{t("System")}</p>
            <h1 className="mt-1 text-[clamp(1.7rem,3vw,2.8rem)] font-semibold leading-[0.95] tracking-[-0.04em] text-ink">
              {t("Settings")}
            </h1>
          </div>
          {(accountName || serverLabel) && (
            <div className="flex min-w-0 items-center gap-2.5 border border-line bg-bg-deep px-3 py-2">
              {accountName && <UserAvatar name={accountName} src={accountAvatar} size={26} />}
              <span className="min-w-0">
                {accountName && (
                  <span className="block max-w-[22ch] truncate text-[12px] text-ink-2">
                    {t("Signed in as {name}", { name: accountName })}
                  </span>
                )}
                {serverLabel && (
                  <span className="mt-0.5 block max-w-[22ch] truncate font-mono text-[9.5px] uppercase tracking-[0.1em] text-ink-3">
                    {t("Server: {name}", { name: serverLabel })}
                  </span>
                )}
              </span>
            </div>
          )}
        </div>

        <p className="mt-2 max-w-[76ch] text-[12.5px] leading-relaxed text-ink-2">{meta.description()}</p>

        <div role="group" aria-label={t("Settings")} className="mt-4 flex items-stretch gap-1 border-b border-line">
          {SETTINGS_SECTIONS.map((id) => {
            const tab = SETTINGS_SECTION_META[id];
            const Icon = tab.icon;
            const selected = id === section;
            return (
              <button
                key={id}
                type="button"
                tabIndex={-1}
                aria-current={selected ? "true" : undefined}
                onClick={() => onSelectSection(id)}
                className={cn(
                  "inline-flex h-10 shrink-0 items-center gap-2 border-b-2 px-2.5 text-[12.5px] transition-colors duration-150",
                  selected
                    ? "border-signal text-ink"
                    : "border-transparent text-ink-3 hover:text-ink-2",
                )}
              >
                <Icon className={cn("size-3.5", selected ? "text-signal-text" : "text-ink-3")} aria-hidden="true" />
                {tab.label()}
              </button>
            );
          })}
          <span className="ms-auto hidden items-center gap-1.5 self-center pb-1 pe-0.5 font-mono text-[9.5px] uppercase tracking-[0.12em] text-ink-3 md:flex">
            <Kbd>LB</Kbd>
            <Kbd>RB</Kbd>
            {t("Tabs")}
          </span>
        </div>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain py-1 bp-pad-x">
        {rows.length === 0 ? (
          <p className="px-1 py-8 text-center text-[13px] text-ink-3">{t("Unavailable")}</p>
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
                    aria-current={selected ? "true" : undefined}
                    aria-disabled={disabled ? "true" : undefined}
                    aria-label={row.value ? `${row.label}: ${row.value}` : row.label}
                    onPointerEnter={() => onHighlight(index)}
                    onClick={() => onActivate(index)}
                    className={cn(
                      "flex min-h-[58px] w-full items-center gap-4 border-b border-line px-2 py-2.5 text-start transition-colors duration-100 sm:min-h-[64px] sm:px-3",
                      selected && "border-signal bg-surface-2",
                      !selected && "hover:bg-surface/50",
                      disabled && "opacity-55",
                    )}
                  >
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-[14px] font-medium text-ink">{row.label}</span>
                      <span className="mt-0.5 block truncate text-[11px] leading-relaxed text-ink-3">
                        {disabled && row.reason ? row.reason : row.detail}
                      </span>
                    </span>
                    <span className="flex shrink-0 items-center justify-end gap-2">
                      {row.kind === "choice" && (
                        <span className="flex items-center gap-1.5 border border-line bg-bg-deep px-2 py-1 text-[12px] text-ink-2">
                          <ChevronLeft className={cn("size-3.5", disabled ? "text-ink-3/50" : "text-ink-3")} aria-hidden="true" />
                          <span className="min-w-[9ch] max-w-[18ch] truncate text-center">{row.value}</span>
                          <ChevronRight className={cn("size-3.5", disabled ? "text-ink-3/50" : "text-ink-3")} aria-hidden="true" />
                        </span>
                      )}
                      {row.kind === "toggle" && (
                        <span
                          className={cn(
                            "border px-2 py-1 font-mono text-[10px] uppercase tracking-[0.1em]",
                            row.value === t("On")
                              ? "border-signal text-signal-text"
                              : "border-line text-ink-3",
                          )}
                        >
                          {row.value}
                        </span>
                      )}
                      {row.kind === "action" && (
                        <span className="flex items-center gap-1.5 font-mono text-[9.5px] uppercase tracking-[0.1em] text-ink-3">
                          {t("Select")}
                          <CornerDownLeft className="size-3.5" aria-hidden="true" />
                        </span>
                      )}
                    </span>
                  </button>
                </li>
              );
            })}
          </ul>
        )}
      </div>

      <dl className="grid shrink-0 grid-cols-2 gap-x-6 gap-y-2 border-t border-line bg-bg-deep/40 py-3 bp-pad-x md:grid-cols-4">
        {[
          { label: t("Version"), value: `v${version}`, numeric: true },
          { label: t("Server"), value: serverLabel ?? t("Unavailable"), numeric: false },
          { label: t("Account"), value: accountName ?? t("Unavailable"), numeric: false },
          { label: t("Controllers"), value: padLabel, numeric: false },
        ].map((fact) => (
          <div key={fact.label} className="min-w-0">
            <dt className="font-mono text-[9px] uppercase tracking-[0.14em] text-ink-3">{fact.label}</dt>
            <dd className={cn("mt-0.5 truncate text-[11.5px] text-ink-2", fact.numeric && "num")}>{fact.value}</dd>
          </div>
        ))}
      </dl>
    </section>
  );
}
