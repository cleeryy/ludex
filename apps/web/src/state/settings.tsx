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
import { scopedStorageKey } from "@/lib/server-storage";
import { normalizeHex } from "@/theme/contrast";
import { applyAccent, applyTint, deriveTheme, tvTokens } from "@/theme/derive";
import {
  contrastReport,
  cssVarsFor,
  isCustomThemeId,
  prefersLight,
} from "@/theme/registry";
import {
  applyAppearance,
  baseThemeFor,
  buildAppearance,
  customThemeById,
  customThemeId,
  defaultConfig,
  modeForSelection,
  readConfig,
  readCustomThemes,
  sanitizeSelection,
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
  ThemeSelection,
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
  accent: string | null;
  tint: string | null;
  customThemes: ThemeFile[];
  contrast: ReturnType<typeof contrastReport>;
  setTheme: (theme: ThemeSelection) => void;
  setAccent: (accent: string | null) => AppearanceResult;
  setTint: (tint: string | null) => AppearanceResult;
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

function sanitizeConfig(config: ThemeConfig, customs: readonly ThemeFile[]): ThemeConfig {
  let selection = sanitizeSelection(config.selection);
  if (isCustomThemeId(selection) && !customThemeById(selection, customs)) selection = "system";
  return { selection, accent: config.accent, tint: config.tint };
}

function baseFor(config: ThemeConfig, customs: readonly ThemeFile[], mode: ThemeMode): BuiltinTheme {
  return baseThemeFor(config.selection, customs, mode);
}

function rejected(issue: ThemeIssue, ratio = 0): AppearanceResult {
  return { ok: false, issue, ratio };
}

export function SettingsProvider({ children }: { children: ReactNode }) {
  const [customThemes, setCustomThemes] = useState<ThemeFile[]>(() => readCustomThemes());
  const [config, setConfig] = useState<ThemeConfig>(() =>
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
    () => deriveTheme(base, config.accent, config.tint).tokens,
    [base, config.accent, config.tint],
  );
  const contrast = useMemo(() => contrastReport(effective), [effective]);

  // The inline boot script already painted the stored variables. Re-applying
  // here keeps React, HMR and tests on the same source of truth. Big Picture
  // swaps in the TV-safe map: 7:1 text, a wider focus ring, no grain.
  const tvAppearance = useMemo(
    () =>
      tv
        ? {
            ...appearance,
            vars: { ...appearance.vars, [resolved]: cssVarsFor(tvTokens(effective)) },
          }
        : appearance,
    [appearance, tv, resolved, effective],
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

  const setTint = useCallback(
    (value: string | null): AppearanceResult => {
      if (!value || value.trim() === "") {
        setConfig((current) => ({ ...current, tint: null }));
        return { ok: true, issue: null, ratio: 0 };
      }
      const tint = normalizeHex(value);
      if (!tint) return rejected({ code: "bad-color", foreground: "tint" });
      const accented = applyAccent(base.tokens, config.accent).tokens;
      const result = applyTint(accented, tint);
      if (result.issue) return rejected(result.issue, result.ratio);
      setConfig((current) => ({ ...current, tint }));
      return { ok: true, issue: null, ratio: result.ratio };
    },
    [base, config.accent],
  );

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
    setConfig({ selection: id, accent: parsed.theme.accent, tint: null });
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
      ),
    );
  }, [config, customThemes, resolved]);

  const setTv = useCallback((enabled: boolean) => setTvState(enabled), []);

  const setView = useCallback((next: LibraryView) => {
    setViewState(next);
    localStorage.setItem(scopedStorageKey(VIEW_KEY), next);
  }, []);

  const value = useMemo(
    () => ({
      theme: config.selection,
      resolved,
      base: base.id,
      accent: config.accent,
      tint: config.tint,
      customThemes,
      contrast,
      setTheme,
      setAccent,
      setTint,
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
      config.tint,
      resolved,
      base.id,
      customThemes,
      contrast,
      setTheme,
      setAccent,
      setTint,
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
  return resolved === "dark" ? "light" : "midnight";
}
