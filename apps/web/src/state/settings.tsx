import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { notifyPreferencesChanged } from "@/lib/preferences-events";
import { scopedStorageKey } from "@/lib/server-storage";
import { normalizeHex } from "@/theme/contrast";
import { applyAccent, applyInfo, applyTint, deriveTheme, tvTokens } from "@/theme/derive";
import {
  contrastReport,
  cssVarsFor,
  isCustomThemeId,
  prefersLight,
} from "@/theme/registry";
import {
  DEFAULT_DARK_THEME_ID,
  DEFAULT_LIGHT_THEME_ID,
  applyAppearance,
  baseThemeFor,
  buildAppearance,
  customThemeById,
  customThemeId,
  defaultConfig,
  modeForSelection,
  readConfig,
  readCustomThemes,
  sanitizeRadius,
  sanitizeSelection,
  sanitizeStripe,
  writeAppearance,
  writeCustomThemes,
} from "@/theme/storage";
import { parseThemeFile, serializeThemeFile, themeFileFor } from "@/theme/validate";
import type {
  BuiltinTheme,
  BuiltinThemeId,
  ThemeConfig,
  ThemeFile,
  ThemeIssue,
  ThemeMode,
  ThemeRadius,
  ThemeSelection,
  ThemeStripe,
  ThemeTokens,
} from "@/theme/types";

export type LibraryView = "grid" | "list";

const VIEW_KEY = "ludex.libraryView";

export interface AppearanceResult {
  ok: boolean;
  issue: ThemeIssue | null;
  ratio: number;
}

export type ImportResult = { ok: true; theme: ThemeFile } | { ok: false; issues: ThemeIssue[] };

interface SettingsValue {
  /** Selected theme id, including `system` and imported `custom:…` ids. */
  theme: ThemeSelection;
  /** Dark/light actually rendered, after resolving the system option. */
  resolved: ThemeMode;
  /** Built-in base palette behind the current selection. */
  base: BuiltinThemeId;
  /** Custom C1 (`--signal`) override. */
  accent: string | null;
  /** Custom C2 (`--info`) override, device-local (#333). */
  info: string | null;
  tint: string | null;
  /** Corner geometry preset applied to `data-radius` (#333). */
  radius: ThemeRadius;
  /** 3px 4-band hardware stripe toggle applied to `data-stripe` (#333). */
  stripe: ThemeStripe;
  /** The token map currently painted at the document root (TV-safe included). */
  tokens: ThemeTokens;
  /** Console's TV-safe variant is on; it enforces 7:1 text. */
  tv: boolean;
  customThemes: ThemeFile[];
  contrast: ReturnType<typeof contrastReport>;
  setTheme: (theme: ThemeSelection) => void;
  /**
   * Applies a whole appearance at once (#201): the selection is sanitised, the
   * accent and tint are validated against the *new* base palette, and an
   * invalid colour is dropped instead of being written back to the account.
   * Device-local #333 controls (`info`, `radius`, `stripe`) are preserved unless
   * explicitly provided.
   */
  setAppearance: (config: ThemeConfig) => void;
  setAccent: (accent: string | null) => AppearanceResult;
  setInfo: (info: string | null) => AppearanceResult;
  setTint: (tint: string | null) => AppearanceResult;
  setRadius: (radius: ThemeRadius) => void;
  setStripe: (stripe: ThemeStripe) => void;
  resetAppearance: () => void;
  importTheme: (source: string) => ImportResult;
  removeCustomTheme: (id: string) => void;
  exportTheme: () => string;
  setTv: (enabled: boolean) => void;
  view: LibraryView;
  setView: (view: LibraryView) => void;
}

const SettingsContext = createContext<SettingsValue | null>(null);

function systemMode(): ThemeMode {
  return prefersLight() ? "light" : "dark";
}

function sanitizeConfig(config: ThemeConfig, customs: readonly ThemeFile[]): Required<ThemeConfig> {
  let selection = sanitizeSelection(config.selection);
  if (isCustomThemeId(selection) && !customThemeById(selection, customs)) selection = "system";
  return {
    selection,
    accent: config.accent,
    tint: config.tint,
    info: config.info ?? null,
    radius: sanitizeRadius(config.radius),
    stripe: sanitizeStripe(config.stripe),
  };
}

function baseFor(config: Required<ThemeConfig>, customs: readonly ThemeFile[], mode: ThemeMode): BuiltinTheme {
  return baseThemeFor(config.selection, customs, mode);
}

function rejected(issue: ThemeIssue, ratio = 0): AppearanceResult {
  return { ok: false, issue, ratio };
}

export function SettingsProvider({ children }: { children: ReactNode }) {
  const [customThemes, setCustomThemes] = useState<ThemeFile[]>(() => readCustomThemes());
  const [config, setConfig] = useState<Required<ThemeConfig>>(() =>
    sanitizeConfig(readConfig(), readCustomThemes()),
  );
  const [system, setSystem] = useState<ThemeMode>(systemMode);
  const [tv, setTvState] = useState(false);
  const [view, setViewState] = useState<LibraryView>(
    () => (localStorage.getItem(scopedStorageKey(VIEW_KEY)) as LibraryView) || "grid",
  );

  const resolved = modeForSelection(config.selection, customThemes, system);
  const appearance = useMemo(() => buildAppearance(config, customThemes), [config, customThemes]);
  const base = baseFor(config, customThemes, resolved);
  const effective = useMemo(
    () => deriveTheme(base, config.accent, config.tint, config.info).tokens,
    [base, config.accent, config.tint, config.info],
  );
  const contrast = useMemo(() => contrastReport(effective), [effective]);
  // The map actually painted at the document root, TV-safe variant included.
  const renderedTokens = useMemo(
    () => (tv ? tvTokens(effective) : effective),
    [tv, effective],
  );

  // The inline boot script already painted the stored variables. Re-applying
  // here keeps React, HMR and tests on the same source of truth. Console mode
  // swaps in the TV-safe map: 7:1 text, a wider focus ring, no grain.
  const tvAppearance = useMemo(
    () =>
      tv
        ? {
            ...appearance,
            vars: { ...appearance.vars, [resolved]: cssVarsFor(renderedTokens) },
          }
        : appearance,
    [appearance, tv, resolved, renderedTokens],
  );

  useLayoutEffect(() => {
    applyAppearance(tvAppearance, resolved, tv);
  }, [tvAppearance, resolved, tv]);

  useEffect(() => {
    writeAppearance(appearance);
  }, [appearance]);

  // Persist every custom theme as soon as the list changes, so an import
  // survives a reload even if the user never touches another setting.
  useEffect(() => {
    writeCustomThemes(customThemes);
  }, [customThemes]);

  useEffect(() => {
    if (config.selection !== "system") return;
    const media = window.matchMedia("(prefers-color-scheme: light)");
    const onChange = () => setSystem(media.matches ? "light" : "dark");
    onChange();
    media.addEventListener("change", onChange);
    return () => media.removeEventListener("change", onChange);
  }, [config.selection]);

  const setTheme = useCallback((next: ThemeSelection) => {
    setConfig((current) => ({ ...current, selection: next }));
  }, []);

  const setAppearance = useCallback(
    (next: ThemeConfig) => {
      setConfig((current) => {
        let selection = sanitizeSelection(next.selection);
        if (isCustomThemeId(selection) && !customThemeById(selection, customThemes)) {
          selection = current.selection;
        }
        const baseTheme = baseThemeFor(
          selection,
          customThemes,
          modeForSelection(selection, customThemes, system),
        );
        const accent = next.accent ? normalizeHex(next.accent) : null;
        const accentResult = accent ? applyAccent(baseTheme.tokens, accent) : null;
        const afterAccent = accentResult && !accentResult.issue ? accentResult.tokens : baseTheme.tokens;

        const candidateInfo = next.info !== undefined ? next.info : current.info;
        const info = candidateInfo ? normalizeHex(candidateInfo) : null;
        const infoResult = info ? applyInfo(afterAccent, info) : null;
        const afterInfo = infoResult && !infoResult.issue ? infoResult.tokens : afterAccent;

        const tint = next.tint ? normalizeHex(next.tint) : null;
        const tintResult = tint ? applyTint(afterInfo, tint) : null;
        return {
          selection,
          accent: accent && (!accentResult || !accentResult.issue) ? accent : null,
          info: info && (!infoResult || !infoResult.issue) ? info : null,
          tint: tint && (!tintResult || !tintResult.issue) ? tint : null,
          radius: next.radius !== undefined ? sanitizeRadius(next.radius) : current.radius,
          stripe: next.stripe !== undefined ? sanitizeStripe(next.stripe) : current.stripe,
        };
      });
    },
    [customThemes, system],
  );

  const setAccent = useCallback(
    (value: string | null): AppearanceResult => {
      if (!value || value.trim() === "") {
        setConfig((current) => ({ ...current, accent: null }));
        return { ok: true, issue: null, ratio: 0 };
      }
      const accent = normalizeHex(value);
      if (!accent) {
        return rejected({ code: "bad-color", foreground: "accent" });
      }
      const result = applyAccent(base.tokens, accent);
      if (result.issue) return rejected(result.issue, result.ratio);
      setConfig((current) => ({ ...current, accent }));
      return { ok: true, issue: null, ratio: result.ratio };
    },
    [base],
  );

  const setInfo = useCallback(
    (value: string | null): AppearanceResult => {
      if (!value || value.trim() === "") {
        setConfig((current) => ({ ...current, info: null }));
        return { ok: true, issue: null, ratio: 0 };
      }
      const info = normalizeHex(value);
      if (!info) {
        return rejected({ code: "bad-color", foreground: "info" });
      }
      const result = applyInfo(base.tokens, info);
      if (result.issue) return rejected(result.issue, result.ratio);
      setConfig((current) => ({ ...current, info }));
      return { ok: true, issue: null, ratio: result.ratio };
    },
    [base],
  );

  const setTint = useCallback(
    (value: string | null): AppearanceResult => {
      if (!value || value.trim() === "") {
        setConfig((current) => ({ ...current, tint: null }));
        return { ok: true, issue: null, ratio: 0 };
      }
      const tint = normalizeHex(value);
      if (!tint) return rejected({ code: "bad-color", foreground: "tint" });
      const accented = applyAccent(base.tokens, config.accent).tokens;
      const informed = applyInfo(accented, config.info).tokens;
      const result = applyTint(informed, tint);
      if (result.issue) return rejected(result.issue, result.ratio);
      setConfig((current) => ({ ...current, tint }));
      return { ok: true, issue: null, ratio: result.ratio };
    },
    [base, config.accent, config.info],
  );

  const setRadius = useCallback((radius: ThemeRadius) => {
    setConfig((current) => ({ ...current, radius: sanitizeRadius(radius) }));
  }, []);

  const setStripe = useCallback((stripe: ThemeStripe) => {
    setConfig((current) => ({ ...current, stripe: sanitizeStripe(stripe) }));
  }, []);

  const resetAppearance = useCallback(() => {
    setConfig(defaultConfig());
  }, []);

  const importTheme = useCallback((source: string): ImportResult => {
    const parsed = parseThemeFile(source);
    if (!parsed.ok) return parsed;
    const id = customThemeId(parsed.theme.name, parsed.theme.tokens);
    setCustomThemes((current) => {
      const next = current.filter((theme) => customThemeId(theme.name, theme.tokens) !== id);
      return [...next, parsed.theme];
    });
    setConfig((current) => ({
      ...current,
      selection: id,
      accent: parsed.theme.accent,
      info: parsed.theme.info ?? null,
      tint: null,
    }));
    return { ok: true, theme: parsed.theme };
  }, []);

  const removeCustomTheme = useCallback((id: string) => {
    setCustomThemes((current) =>
      current.filter((theme) => customThemeId(theme.name, theme.tokens) !== id),
    );
    setConfig((current) =>
      current.selection === id ? { ...current, selection: "system" } : current,
    );
  }, []);

  const exportTheme = useCallback((): string => {
    const currentBase = baseFor(config, customThemes, resolved);
    const custom = isCustomThemeId(config.selection)
      ? customThemeById(config.selection, customThemes)
      : null;
    return serializeThemeFile(
      themeFileFor(
        custom?.name ?? currentBase.label,
        currentBase,
        custom?.tokens ?? currentBase.tokens,
        config.accent,
        config.info,
      ),
    );
  }, [config, customThemes, resolved]);

  const setTv = useCallback((enabled: boolean) => setTvState(enabled), []);

  const setView = useCallback((next: LibraryView) => {
    setViewState(next);
    localStorage.setItem(scopedStorageKey(VIEW_KEY), next);
    notifyPreferencesChanged();
  }, []);

  const value = useMemo(
    () => ({
      theme: config.selection,
      resolved,
      base: base.id,
      accent: config.accent,
      info: config.info,
      tint: config.tint,
      radius: config.radius,
      stripe: config.stripe,
      tokens: renderedTokens,
      tv,
      customThemes,
      contrast,
      setTheme,
      setAppearance,
      setAccent,
      setInfo,
      setTint,
      setRadius,
      setStripe,
      resetAppearance,
      importTheme,
      removeCustomTheme,
      exportTheme,
      setTv,
      view,
      setView,
    }),
    [
      config.selection,
      config.accent,
      config.info,
      config.tint,
      config.radius,
      config.stripe,
      resolved,
      renderedTokens,
      tv,
      base.id,
      customThemes,
      contrast,
      setTheme,
      setAppearance,
      setAccent,
      setInfo,
      setTint,
      setRadius,
      setStripe,
      resetAppearance,
      importTheme,
      removeCustomTheme,
      exportTheme,
      setTv,
      view,
      setView,
    ],
  );

  return <SettingsContext.Provider value={value}>{children}</SettingsContext.Provider>;
}

export function useSettings(): SettingsValue {
  const value = useContext(SettingsContext);
  if (!value) throw new Error("useSettings must be used inside SettingsProvider");
  return value;
}

/** Quick toggle used by the account menu and the command palette. */
export function otherModeTheme(resolved: ThemeMode): BuiltinThemeId {
  return resolved === "dark" ? DEFAULT_LIGHT_THEME_ID : DEFAULT_DARK_THEME_ID;
}
