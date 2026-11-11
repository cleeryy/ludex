import { useEffect, useRef, useState } from "react";
import { Check, Download, RotateCcw, Upload, X } from "lucide-react";
import { cn } from "@/lib/cn";
import { t } from "@/lib/i18n";
import { useSettings } from "@/state/settings";
import { useToast } from "@/state/toasts";
import { Badge, Button, Textarea } from "@/components/ui/controls";
import { BigPictureLayoutPanel } from "@/components/big-picture/layout-panel";
import { AA_FOCUS, AA_TEXT, ensureContrast, normalizeHex } from "@/theme/contrast";
import { applyAccent, applyTint } from "@/theme/derive";
import { BUILTIN_THEMES, builtinTheme, cssVarsFor, isBuiltinThemeId } from "@/theme/registry";
import { customThemeId } from "@/theme/storage";
import type { BuiltinThemeId, ThemeIssue, ThemeTokens } from "@/theme/types";

/**
 * Settings → Appearance (#140).
 *
 * Theme cards render a miniature of the real interface with the theme's own
 * custom properties, so the preview cannot drift from the result. Every action
 * applies immediately; imports only replace the active theme once every token,
 * hex value and contrast pair has passed.
 */

function themeLabel(id: BuiltinThemeId): string {
  switch (id) {
    case "midnight":
      return t("Midnight");
    case "light":
      return t("Paper");
    case "nord":
      return t("Nord");
    case "gruvbox":
      return t("Gruvbox");
    case "amber":
      return t("Amber");
  }
}

function modeLabel(mode: "dark" | "light"): string {
  return mode === "dark" ? t("Dark") : t("Light");
}

function issueMessage(issue: ThemeIssue): string {
  const ratio = issue.ratio !== undefined ? issue.ratio.toFixed(2) : "";
  const min = issue.min !== undefined ? issue.min.toFixed(1) : "";
  switch (issue.code) {
    case "bad-format":
      return issue.field === "json"
        ? t("The file is not valid JSON.")
        : t("The theme file could not be read. Export a theme from Ludex and try again.");
    case "bad-field":
      return t("The theme file is missing its token map.");
    case "bad-name":
      return t("The theme name must be between 1 and 48 characters.");
    case "bad-base":
      return t("“{base}” is not a base theme. Use midnight, light, nord, gruvbox or amber.", {
        base: issue.base ?? "",
      });
    case "bad-mode":
      return t("The theme mode does not match its base theme.");
    case "bad-color":
      return issue.foreground === "accent"
        ? t("The accent is not a hex colour with three or six digits.")
        : t("“{token}” is not a hex colour with three or six digits.", { token: issue.token ?? "" });
    case "bad-grain":
      return t("“grain-opacity” must be a number between 0 and 0.2.");
    case "missing-token":
      return t("The theme is missing the “{token}” token.", { token: issue.token ?? "" });
    case "unknown-token":
      return t("Unknown token “{token}”. Remove it from the file.", { token: issue.token ?? "" });
    case "low-contrast":
      if (issue.foreground === "accent" && issue.background === "signal") {
        return t("Text on this accent is only {ratio}:1. Pick a darker or lighter accent.", { ratio });
      }
      if (issue.foreground === "accent") {
        return t("This accent is only {ratio}:1 against the background. Pick a colour with at least {min}:1.", {
          ratio,
          min,
        });
      }
      if (issue.foreground === "tint") {
        return t("This tint drops text contrast to {ratio}:1 on the surfaces. Pick a darker or lighter tint; {min}:1 is required.", {
          ratio,
          min,
        });
      }
      return t("“{foreground}” on “{background}” is {ratio}:1; it needs {min}:1.", {
        foreground: issue.foreground ?? "",
        background: issue.background ?? "",
        ratio,
        min,
      });
  }
}

/* --------------------------------- preview -------------------------------- */

function MiniScreen({ tokens }: { tokens: ThemeTokens }) {
  return (
    <div className="flex h-[86px] flex-col bg-bg" style={cssVarsFor(tokens)} aria-hidden="true">
      <div className="flex h-3.5 items-center gap-1 border-b border-line bg-surface px-1.5">
        <span className="h-1 w-4 bg-ink" />
        <span className="ms-auto h-1 w-3 bg-ink-3" />
      </div>
      <div className="flex min-h-0 flex-1">
        <div className="flex w-[24%] flex-col gap-1 border-e border-line bg-bg-deep p-1.5">
          <span className="h-1 w-full bg-signal" />
          <span className="h-1 w-3/4 bg-line-2" />
          <span className="h-1 w-2/3 bg-line-2" />
        </div>
        <div className="flex min-w-0 flex-1 flex-col justify-between p-1.5">
          <div className="flex flex-col gap-1">
            <span className="h-1.5 w-8 bg-ink" />
            <span className="h-1 w-11 bg-ink-2" />
            <span className="h-1 w-7 bg-ink-3" />
          </div>
          <div className="flex items-end justify-between gap-1">
            <span className="flex h-3.5 w-11 items-center justify-center bg-signal">
              <span className="h-1 w-5 bg-on-signal" />
            </span>
            <span
              className="h-8 w-6 rounded-[1px] border border-line-2"
              style={{ background: "linear-gradient(150deg, var(--signal), var(--surface-2) 72%)" }}
            />
          </div>
        </div>
      </div>
    </div>
  );
}

function SystemPreview() {
  return (
    <div className="grid grid-cols-2 gap-px bg-line-2" aria-hidden="true">
      <MiniScreen tokens={builtinTheme("midnight").tokens} />
      <MiniScreen tokens={builtinTheme("light").tokens} />
    </div>
  );
}

function ThemeCard({
  label,
  mode,
  tokens,
  selected,
  onSelect,
  system = false,
  onRemove,
}: {
  label: string;
  mode: "dark" | "light";
  tokens?: ThemeTokens;
  selected: boolean;
  onSelect: () => void;
  system?: boolean;
  onRemove?: () => void;
}) {
  return (
    <div className="relative">
      <button
        type="button"
        onClick={onSelect}
        aria-pressed={selected}
        aria-label={`${label} — ${modeLabel(mode)}`}
        className={cn(
          "flex w-full flex-col gap-1.5 border p-1.5 text-start transition-colors",
          selected
            ? "border-signal bg-surface-2"
            : "border-line hover:border-line-2 hover:bg-surface",
        )}
      >
        <div className="overflow-hidden rounded-[1px] border border-line-2">
          {system || !tokens ? <SystemPreview /> : <MiniScreen tokens={tokens} />}
        </div>
        <span className="flex items-start justify-between gap-2 px-0.5 pb-0.5">
          <span className="min-w-0">
            <span className="block truncate text-[12.5px] font-medium leading-tight text-ink">
              {label}
            </span>
            <span className="mt-0.5 block truncate font-mono text-[9.5px] uppercase tracking-[0.12em] text-ink-3">
              {modeLabel(mode)}
            </span>
          </span>
          <span
            className={cn(
              "mt-0.5 grid size-4 shrink-0 place-items-center rounded-full border",
              selected ? "border-signal bg-signal text-on-signal" : "border-line-2 text-transparent",
            )}
          >
            <Check className="size-2.5" strokeWidth={3} aria-hidden="true" />
          </span>
        </span>
      </button>
      {onRemove && (
        <button
          type="button"
          onClick={onRemove}
          aria-label={t("Remove {name}", { name: label })}
          title={t("Remove {name}", { name: label })}
          className="absolute end-2 top-2 grid size-5 place-items-center rounded-[2px] border border-line-2 bg-bg-deep/85 text-ink-2 transition-colors hover:border-danger hover:text-danger"
        >
          <X className="size-3" aria-hidden="true" />
        </button>
      )}
    </div>
  );
}

/* ---------------------------------- panel --------------------------------- */

export function AppearancePanel() {
  const {
    theme,
    resolved,
    accent,
    tint,
    customThemes,
    contrast,
    setTheme,
    setAccent,
    setTint,
    resetAppearance,
    importTheme,
    removeCustomTheme,
    exportTheme,
  } = useSettings();
  const { push } = useToast();

  const [accentDraft, setAccentDraft] = useState(accent ?? "");
  const [accentIssue, setAccentIssue] = useState<ThemeIssue | null>(null);
  const [accentRatio, setAccentRatio] = useState(contrast.accent);
  const [tintIssue, setTintIssue] = useState<ThemeIssue | null>(null);
  const [tintRatio, setTintRatio] = useState(0);
  const [importIssues, setImportIssues] = useState<ThemeIssue[]>([]);
  const [paste, setPaste] = useState("");
  const fileRef = useRef<HTMLInputElement>(null);

  const custom = customThemes.find((entry) => customThemeId(entry.name, entry.tokens) === theme);
  const baseId: BuiltinThemeId = custom
    ? custom.base
    : theme === "system"
      ? resolved === "light"
        ? "light"
        : "midnight"
      : isBuiltinThemeId(theme)
        ? theme
        : "midnight";
  const base = builtinTheme(baseId);

  const accented = applyAccent(base.tokens, accent).tokens;
  const activeAccentIssue = accent && !accentIssue ? applyAccent(base.tokens, accent).issue : null;
  const activeTintIssue = tint && !tintIssue ? applyTint(accented, tint).issue : null;
  const shownAccentIssue = accentIssue ?? activeAccentIssue;
  const shownTintIssue = tintIssue ?? activeTintIssue;
  const currentAccentRatio = accent ? applyAccent(base.tokens, accent).ratio : contrast.accent;
  const currentTintRatio = tint ? applyTint(accented, tint).ratio : 0;
  const shownAccentRatio = accentIssue ? accentRatio : currentAccentRatio;
  const shownTintRatio = tintIssue ? tintRatio : currentTintRatio;

  useEffect(() => {
    setAccentDraft(accent ?? "");
    setAccentIssue(null);
    setAccentRatio(contrast.accent);
  }, [accent, contrast.accent]);

  const tryAccent = (value: string) => {
    setAccentDraft(value);
    if (value.trim() === "") {
      setAccent(null);
      setAccentIssue(null);
      setAccentRatio(contrast.accent);
      return;
    }
    const normalized = normalizeHex(value) ?? value;
    const result = setAccent(normalized);
    setAccentIssue(result.ok ? null : result.issue);
    if (result.ratio) setAccentRatio(result.ratio);
  };

  const tryTint = (value: string) => {
    if (value.trim() === "") {
      setTint(null);
      setTintIssue(null);
      setTintRatio(0);
      return;
    }
    const result = setTint(value);
    setTintIssue(result.ok ? null : result.issue);
    setTintRatio(result.ratio);
  };

  const accentPresets = Array.from(
    new Set(
      BUILTIN_THEMES.map((entry) =>
        ensureContrast(
          entry.tokens.signal,
          [base.tokens.bg, base.tokens.surface, base.tokens["surface-2"], base.tokens.elevated],
          AA_TEXT,
        ),
      ),
    ),
  ).filter((color) => !applyAccent(base.tokens, color).issue);

  const runImport = (source: string) => {
    const result = importTheme(source);
    if (result.ok) {
      setImportIssues([]);
      setPaste("");
      push({
        title: t("Theme imported"),
        description: t("“{name}” is now the active theme.", { name: result.theme.name }),
      });
      return;
    }
    setImportIssues(result.issues);
    push({ title: t("Could not import the theme"), tone: "danger" });
  };

  const onFile = (event: React.ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    event.target.value = "";
    if (!file) return;
    void file
      .text()
      .then((source) => runImport(source))
      .catch(() => setImportIssues([{ code: "bad-format", field: "json" }]));
  };

  const exportCurrent = () => {
    const source = exportTheme();
    const url = URL.createObjectURL(new Blob([source], { type: "application/json" }));
    const anchor = document.createElement("a");
    const slug = (custom?.name ?? base.label).toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");
    anchor.href = url;
    anchor.download = `ludex-theme-${slug || "theme"}.json`;
    anchor.click();
    window.setTimeout(() => URL.revokeObjectURL(url), 1_000);
    push({ title: t("Theme exported") });
  };

  return (
    <div className="flex flex-col gap-7">
      <div>
        <div className="mb-3 flex flex-wrap items-baseline justify-between gap-3">
          <div>
            <p className="text-[13px] font-medium text-ink">{t("Theme")}</p>
            <p className="mt-1 text-[11.5px] leading-relaxed text-ink-3">
              {t("Pick a theme. Changes apply immediately, in the library and in Big Picture.")}
            </p>
          </div>
          <span className="font-mono text-[10px] uppercase tracking-[0.12em] text-ink-3">
            {t("Now: {theme}", {
              theme: theme === "system" ? t("Follow the system") : (custom?.name ?? themeLabel(base.id)),
            })}
          </span>
        </div>

        <div className="grid grid-cols-2 gap-2.5 sm:grid-cols-3 xl:grid-cols-6">
          <ThemeCard
            label={t("Follow the system")}
            mode={resolved}
            selected={theme === "system"}
            onSelect={() => setTheme("system")}
            system
          />
          {BUILTIN_THEMES.map((entry) => (
            <ThemeCard
              key={entry.id}
              label={themeLabel(entry.id)}
              mode={entry.mode}
              tokens={entry.tokens}
              selected={theme === entry.id}
              onSelect={() => setTheme(entry.id)}
            />
          ))}
          {customThemes.map((entry) => {
            const id = customThemeId(entry.name, entry.tokens);
            return (
              <ThemeCard
                key={id}
                label={entry.name}
                mode={entry.mode}
                tokens={entry.tokens}
                selected={theme === id}
                onSelect={() => setTheme(id)}
                onRemove={() => removeCustomTheme(id)}
              />
            );
          })}
        </div>
        <p className="mt-2 text-[11.5px] leading-relaxed text-ink-3">
          {t("The system option follows your OS light and dark setting and reacts when it changes.")}
        </p>
      </div>

      <div className="grid gap-4 lg:grid-cols-2">
        <section className="border border-line p-4">
          <div className="flex flex-wrap items-start justify-between gap-3">
            <div>
              <p className="text-[13px] font-medium text-ink">{t("Custom accent")}</p>
              <p className="mt-1 max-w-[42ch] text-[11.5px] leading-relaxed text-ink-3">
                {t("Used for buttons, selection markers and the focus ring.")}
              </p>
            </div>
            <div className="flex items-center gap-1.5">
              {accentPresets.map((color, index) => (
                <button
                  key={`${color}-${index}`}
                  type="button"
                  onClick={() => tryAccent(color)}
                  aria-label={t("Use accent {index}", { index: index + 1 })}
                  className={cn(
                    "size-6 rounded-full border transition-transform hover:scale-110",
                    accent === color ? "border-2" : "border-line-2",
                  )}
                  style={{ backgroundColor: color, borderColor: accent === color ? "var(--focus)" : undefined }}
                />
              ))}
              <label
                className="relative grid size-6 cursor-pointer place-items-center rounded-full border border-dashed border-line-2 text-ink-3 hover:text-ink"
                title={t("Pick an accent colour")}
              >
                <input
                  type="color"
                  value={accent ?? base.tokens.signal}
                  onChange={(event) => tryAccent(event.target.value)}
                  aria-label={t("Pick an accent colour")}
                  className="absolute inset-0 size-full cursor-pointer opacity-0"
                />
                <span aria-hidden="true" className="font-mono text-[11px]">
                  +
                </span>
              </label>
            </div>
          </div>

          <div className="mt-3 flex flex-wrap items-center gap-2.5">
            <label className="flex h-8 items-center gap-2 border border-line-2 bg-surface px-2 font-mono text-[11px] text-ink-2">
              <span aria-hidden="true" className="text-ink-3">
                #
              </span>
              <input
                value={accentDraft.replace(/^#/, "")}
                onChange={(event) => {
                  const digits = event.target.value.replace(/[^0-9a-fA-F]/g, "").slice(0, 6);
                  const next = `#${digits}`;
                  setAccentDraft(next);
                  if (digits.length === 3 || digits.length === 6) tryAccent(next);
                }}
                aria-label={t("Accent hex")}
                placeholder="rrggbb"
                spellCheck={false}
                className="w-[70px] bg-transparent text-ink outline-none"
              />
            </label>
            <Badge tone={shownAccentIssue ? "danger" : "ok"}>
              {shownAccentIssue
                ? t("Needs {min}:1", { min: (shownAccentIssue.min ?? AA_TEXT).toFixed(1) })
                : t("{ratio}:1 AA", { ratio: shownAccentRatio.toFixed(2) })}
            </Badge>
            <Button size="sm" variant="ghost" onClick={() => tryAccent("")} disabled={!accent}>
              {t("Clear")}
            </Button>
          </div>
          {shownAccentIssue && (
            <p role="alert" className="mt-2 border-l-2 border-danger pl-2.5 text-[12px] leading-relaxed text-danger">
              {issueMessage(shownAccentIssue)}
            </p>
          )}
          <p className="mt-2 text-[11px] leading-relaxed text-ink-3">
            {t("Accents below {min}:1 against the background are refused; the measured ratio is shown above.", {
              min: AA_TEXT.toFixed(1),
            })}
          </p>
        </section>

        <section className="border border-line p-4">
          <div className="flex flex-wrap items-start justify-between gap-3">
            <div>
              <p className="text-[13px] font-medium text-ink">{t("Background tint")}</p>
              <p className="mt-1 max-w-[42ch] text-[11.5px] leading-relaxed text-ink-3">
                {t("A subtle wash over the base theme's surfaces. Text contrast is re-checked.")}
              </p>
            </div>
            <div className="flex items-center gap-1.5">
              <label
                className="relative size-6 cursor-pointer overflow-hidden rounded-full border border-line-2"
                title={t("Pick a tint colour")}
                style={{ backgroundColor: tint ?? "var(--surface-2)" }}
              >
                <input
                  type="color"
                  value={tint ?? base.tokens.surface}
                  onChange={(event) => tryTint(event.target.value)}
                  aria-label={t("Pick a tint colour")}
                  className="absolute inset-0 size-full cursor-pointer opacity-0"
                />
              </label>
              <Button size="sm" variant="ghost" onClick={() => tryTint("")} disabled={!tint}>
                {t("Clear")}
              </Button>
            </div>
          </div>
          <div className="mt-3 flex flex-wrap items-center gap-2.5">
            <Badge tone={shownTintIssue ? "danger" : tint ? "ok" : "outline"}>
              {tint
                ? shownTintIssue
                  ? t("Needs {min}:1", { min: (shownTintIssue.min ?? AA_TEXT).toFixed(1) })
                  : t("{ratio}:1 on surfaces", { ratio: shownTintRatio.toFixed(2) })
                : t("No tint")}
            </Badge>
            <span className="font-mono text-[10.5px] text-ink-3">{tint ?? t("Base surfaces")}</span>
          </div>
          {shownTintIssue && (
            <p role="alert" className="mt-2 border-l-2 border-danger pl-2.5 text-[12px] leading-relaxed text-danger">
              {issueMessage(shownTintIssue)}
            </p>
          )}
        </section>
      </div>

      <section className="border border-line p-4">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div>
            <p className="text-[13px] font-medium text-ink">{t("Theme file")}</p>
            <p className="mt-1 max-w-[64ch] text-[11.5px] leading-relaxed text-ink-3">
              {t("Export the current theme as JSON, or import one. Unknown tokens, invalid colours and low-contrast pairs are rejected with the measured value.")}
            </p>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <Button size="sm" variant="default" icon={<Download className="size-3.5" />} onClick={exportCurrent}>
              {t("Export")}
            </Button>
            <Button
              size="sm"
              variant="default"
              icon={<Upload className="size-3.5" />}
              onClick={() => fileRef.current?.click()}
            >
              {t("Import")}
            </Button>
            <input
              ref={fileRef}
              type="file"
              accept="application/json,.json"
              onChange={onFile}
              className="sr-only"
              aria-label={t("Theme file to import")}
            />
          </div>
        </div>

        <div className="mt-3 flex flex-wrap items-center gap-2 font-mono text-[10.5px] text-ink-3">
          <span>{t("Base: {base}", { base: themeLabel(base.id) })}</span>
          <span aria-hidden="true">·</span>
          <span>{t("Accent: {accent}", { accent: accent ?? t("theme default") })}</span>
        </div>

        <details className="mt-3">
          <summary className="cursor-pointer text-[12px] text-ink-2 hover:text-ink">
            {t("Or paste a theme file")}
          </summary>
          <div className="mt-2 flex flex-col gap-2">
            <Textarea
              value={paste}
              onChange={(event) => setPaste(event.target.value)}
              aria-label={t("Theme JSON")}
              spellCheck={false}
              rows={5}
              className="min-h-[120px] font-mono text-[11.5px]"
              placeholder={'{\n  "name": "…",\n  "base": "midnight"\n}'}
            />
            <div>
              <Button size="sm" variant="default" onClick={() => runImport(paste)} disabled={!paste.trim()}>
                {t("Import pasted theme")}
              </Button>
            </div>
          </div>
        </details>

        {importIssues.length > 0 && (
          <div role="alert" className="mt-3 border-l-2 border-danger bg-danger/5 py-2 pl-3">
            <p className="text-[12.5px] font-medium text-danger">{t("The theme was not imported")}</p>
            <ul className="mt-1.5 flex list-inside list-disc flex-col gap-1 text-[11.5px] leading-relaxed text-ink-2">
              {importIssues.slice(0, 6).map((issue, index) => (
                <li key={`${issue.code}-${issue.token ?? issue.field ?? index}`}>{issueMessage(issue)}</li>
              ))}
            </ul>
            {importIssues.length > 6 && (
              <p className="mt-1 text-[11px] text-ink-3">
                {t("and {count} more problems", { count: importIssues.length - 6 })}
              </p>
            )}
            <p className="mt-1.5 text-[11px] leading-relaxed text-ink-3">
              {t("The previous theme stays active until the file passes every check.")}
            </p>
          </div>
        )}
      </section>

      <BigPictureLayoutPanel />

      <section className="border-t border-line pt-4">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <p className="text-[11.5px] leading-relaxed text-ink-3">
            {t("Big Picture uses the same theme with TV contrast. Focus rings stay visible and decorative text is lifted to 7:1.")}
          </p>
          <Button size="sm" variant="ghost" icon={<RotateCcw className="size-3.5" />} onClick={() => {
            resetAppearance();
            setAccentIssue(null);
            setTintIssue(null);
            setImportIssues([]);
            push({ title: t("Appearance reset") });
          }}>
            {t("Reset appearance")}
          </Button>
        </div>

        <div className="mt-4 grid gap-x-8 gap-y-2 sm:grid-cols-2 lg:grid-cols-4">
          {[
            { label: t("Body text"), value: contrast.body, min: AA_TEXT },
            { label: t("Secondary text"), value: contrast.secondary, min: AA_TEXT },
            { label: t("Muted text"), value: contrast.muted, min: AA_TEXT },
            { label: t("Focus ring"), value: contrast.focus, min: AA_FOCUS },
          ].map((check) => (
            <div key={check.label} className="flex items-baseline justify-between gap-2 border-b border-line pb-1.5">
              <span className="text-[11.5px] text-ink-3">{check.label}</span>
              <span className="flex items-baseline gap-1.5">
                <span className="num text-[11.5px] text-ink">{check.value.toFixed(2)}:1</span>
                <Badge tone={check.value >= check.min ? "ok" : "danger"}>
                  {check.value >= check.min ? "AA" : "!"}
                </Badge>
              </span>
            </div>
          ))}
        </div>
      </section>
    </div>
  );
}
