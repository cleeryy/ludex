import {
  createContext,
  Fragment,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import type enCatalog from "../locales/en.json";
import coverageData from "../locales/coverage.json";
import localeManifest from "../locales/manifest.json";

/**
 * Ludex i18n. English is the source language, so the English message id *is*
 * the copy ("gettext" style). Locale files are flat JSON maps keyed by that
 * message id; a missing entry falls back to English. Plurals use a `|category`
 * suffix on the message id (`Intl.PluralRules` categories), so a translator can
 * add `|one`, `|few`, `|many`, ... without touching the code.
 *
 * `apps/web/scripts/i18n.mjs` owns the catalog: it extracts every `t("…")` and
 * `tn(…, "one", "other")` from the source, writes `src/locales/en.json`, checks
 * key parity, placeholders and freshness of `coverage.json`.
 */

export type LocaleCode = string;
export type LocaleDir = "ltr" | "rtl";
export type LocaleStatus = "official" | "community";

export interface LocaleMeta {
  code: LocaleCode;
  /** Native name, shown in the picker. */
  label: string;
  englishLabel: string;
  dir: LocaleDir;
  status: LocaleStatus;
  /** Community translations start unreviewed; Arabic is machine-seeded. */
  reviewed: boolean;
  /** Intl locale used for dates, numbers and relative times. */
  intl: string;
}

export const LOCALE_STORAGE_KEY = "ludex.locale";
export const DEFAULT_LOCALE: LocaleCode = "en";

export const LOCALES = localeManifest as readonly LocaleMeta[];
export const LOCALE_CODES: LocaleCode[] = LOCALES.map((locale) => locale.code);
const LOCALE_BY_CODE = new Map(LOCALES.map((locale) => [locale.code, locale]));

export type Messages = Record<string, string>;
export type MessageVars = Record<string, string | number | null | undefined>;
/** Compile-time catalog of every message id used in the source. */
export type MessageKey = keyof typeof enCatalog;

/**
 * Every locale file is discovered at build time. Adding a language means
 * dropping `src/locales/xx.json` and adding an entry to `manifest.json`; no
 * TypeScript changes are needed.
 */
const localeModules = import.meta.glob<{ default: Messages }>([
  "../locales/*.json",
  "!../locales/en.json",
  "!../locales/manifest.json",
  "!../locales/coverage.json",
]);

function isLocaleCode(value: string | null | undefined): value is LocaleCode {
  return Boolean(value && LOCALE_CODES.includes(value));
}

export function localeMeta(code: LocaleCode): LocaleMeta {
  return LOCALE_BY_CODE.get(code) ?? LOCALE_BY_CODE.get(DEFAULT_LOCALE) ?? LOCALES[0];
}

export function isRtl(code: LocaleCode = activeLocale): boolean {
  return localeMeta(code).dir === "rtl";
}

export function getStoredLocale(): LocaleCode {
  try {
    const stored = localStorage.getItem(LOCALE_STORAGE_KEY);
    return isLocaleCode(stored) ? stored : DEFAULT_LOCALE;
  } catch {
    return DEFAULT_LOCALE;
  }
}

let activeLocale: LocaleCode = DEFAULT_LOCALE;
let activeMessages: Messages = {};
let hydrationToken = 0;
let activeCoverage: Coverage = coverageData as Coverage;

async function loadMessages(code: LocaleCode): Promise<Messages> {
  if (code === DEFAULT_LOCALE) return {};
  const loader = localeModules[`../locales/${code}.json`];
  if (!loader) return {};
  try {
    const module = await loader();
    return (module.default ?? {}) as Messages;
  } catch {
    return {};
  }
}

function applyDocumentLocale(code: LocaleCode): void {
  if (typeof document === "undefined") return;
  const root = document.documentElement;
  root.lang = code;
  root.dir = isRtl(code) ? "rtl" : "ltr";
  root.dataset.locale = code;
}

/** Loads the persisted locale before the first React render (see main.tsx). */
export async function bootstrapI18n(): Promise<void> {
  const code = getStoredLocale();
  const messages = await loadMessages(code);
  activeLocale = code;
  activeMessages = messages;
  applyDocumentLocale(code);
}

/** Applied before paint by the inline boot script in index.html. */
export function getLocaleFromStorageUnsafe(): LocaleCode {
  try {
    const stored = window.localStorage.getItem(LOCALE_STORAGE_KEY);
    return isLocaleCode(stored) ? stored : DEFAULT_LOCALE;
  } catch {
    return DEFAULT_LOCALE;
  }
}

export function getLocale(): LocaleCode {
  return activeLocale;
}

export function getIntlLocale(code: LocaleCode = activeLocale): string {
  return localeMeta(code).intl;
}

function interpolate(template: string, vars?: MessageVars): string {
  if (!vars) return template;
  return template.replace(/\{(\w+)\}/g, (whole, name: string) => {
    const value = vars[name];
    if (value === undefined || value === null) return whole;
    return typeof value === "number" ? formatNumber(value) : String(value);
  });
}

function resolve(key: string, vars?: MessageVars): string {
  if (typeof vars?.count === "number" && Number.isFinite(vars.count)) {
    const category = pluralCategory(vars.count);
    const suffixed = activeMessages[`${key}|${category}`] ?? activeMessages[`${key}|other`];
    if (suffixed !== undefined) return suffixed;
  }
  return activeMessages[key] ?? key;
}

/** Translate one message id, interpolating `{name}` placeholders. */
export function t(key: MessageKey, vars?: MessageVars): string {
  return interpolate(resolve(key, vars), vars);
}

/**
 * Plural-aware translate. `one` is used for the English singular fallback and
 * `other` for the plural; translators override either with `|category` keys.
 */
export function tn(count: number, one: MessageKey, other: MessageKey, vars?: MessageVars): string {
  const merged = { ...vars, count };
  const category = pluralCategory(count);
  const suffixed = activeMessages[`${other}|${category}`] ?? activeMessages[`${other}|other`];
  if (suffixed !== undefined) return interpolate(suffixed, merged);
  if (count === 1) {
    const singular = activeMessages[one] ?? activeMessages[`${one}|${category}`];
    if (singular !== undefined) return interpolate(singular, merged);
    return interpolate(one, merged);
  }
  return interpolate(activeMessages[other] ?? other, merged);
}

/** Rich-text translation: `{name}` values may be React nodes. */
export function tr(key: MessageKey, vars: Record<string, ReactNode>): ReactNode {
  const template = activeMessages[key] ?? key;
  return template.split(/(\{\w+\})/g).map((part, index) => {
    const match = /^\{(\w+)\}$/.exec(part);
    return <Fragment key={index}>{match ? (vars[match[1]] ?? part) : part}</Fragment>;
  });
}

export function pluralCategory(count: number, code: LocaleCode = activeLocale): string {
  return new Intl.PluralRules(getIntlLocale(code)).select(count);
}

export function formatNumber(value: number, options?: Intl.NumberFormatOptions, code: LocaleCode = activeLocale): string {
  return new Intl.NumberFormat(getIntlLocale(code), options).format(value);
}

export function formatDateValue(
  value: Date | string | number,
  options: Intl.DateTimeFormatOptions = { day: "numeric", month: "short", year: "numeric" },
  code: LocaleCode = activeLocale,
): string {
  const date = value instanceof Date ? value : new Date(value);
  if (Number.isNaN(date.getTime())) return "—";
  return new Intl.DateTimeFormat(getIntlLocale(code), options).format(date);
}

export function formatRelativeValue(
  value: Date | string | number,
  unit: Intl.RelativeTimeFormatUnit,
  code: LocaleCode = activeLocale,
): string {
  return new Intl.RelativeTimeFormat(getIntlLocale(code), { numeric: "auto" }).format(
    typeof value === "number" ? value : 0,
    unit,
  );
}

export function formatListValue(values: readonly string[], code: LocaleCode = activeLocale): string {
  return new Intl.ListFormat(getIntlLocale(code), { style: "long", type: "conjunction" }).format(values);
}

export function formatRelativeTime(
  iso: string | null | undefined,
  now = Date.now(),
  code: LocaleCode = activeLocale,
): string {
  if (!iso) return t("Never");
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return t("Never");
  const minutes = Math.round((then - now) / 60_000);
  const formatter = new Intl.RelativeTimeFormat(getIntlLocale(code), { numeric: "auto" });
  if (Math.abs(minutes) < 60) return formatter.format(minutes, "minute");
  const hours = Math.round(minutes / 60);
  if (Math.abs(hours) < 24) return formatter.format(hours, "hour");
  const days = Math.round(hours / 24);
  if (Math.abs(days) < 30) return formatter.format(days, "day");
  const months = Math.round(days / 30);
  if (Math.abs(months) < 12) return formatter.format(months, "month");
  return formatter.format(Math.round(months / 12), "year");
}

/* ------------------------------ provider -------------------------------- */

interface I18nValue {
  locale: LocaleCode;
  meta: LocaleMeta;
  dir: LocaleDir;
  /** Changes the locale, loading its catalog first so the tree remounts once. */
  setLocale: (code: LocaleCode) => Promise<void>;
}

const I18nContext = createContext<I18nValue | null>(null);

export function I18nProvider({ children }: { children: ReactNode }) {
  const [locale, setLocaleState] = useState<LocaleCode>(() => activeLocale);

  useEffect(() => {
    // bootstrapI18n already ran in main.tsx; this covers tests and HMR.
    if (activeLocale !== locale) void hydrate(locale, setLocaleState);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const setLocale = useCallback(async (next: LocaleCode) => {
    if (next === activeLocale) return;
    await hydrate(next, setLocaleState);
  }, []);

  const value = useMemo<I18nValue>(
    () => ({ locale, meta: localeMeta(locale), dir: localeMeta(locale).dir, setLocale }),
    [locale, setLocale],
  );

  return (
    <I18nContext.Provider value={value}>
      {/* Locale changes remount the tree: copy comes from module-level `t`,
          which is not a hook, so a keyed remount is the cheapest correct
          update. Providers above this one keep their state. */}
      <Fragment key={locale}>{children}</Fragment>
    </I18nContext.Provider>
  );
}

async function hydrate(code: LocaleCode, set: (code: LocaleCode) => void): Promise<void> {
  const token = ++hydrationToken;
  const messages = await loadMessages(code);
  if (token !== hydrationToken) return;
  activeLocale = code;
  activeMessages = messages;
  try {
    localStorage.setItem(LOCALE_STORAGE_KEY, code);
  } catch {
    /* private mode: the preference is session-only */
  }
  applyDocumentLocale(code);
  set(code);
}

export function useI18n(): I18nValue {
  const value = useContext(I18nContext);
  if (!value) throw new Error("useI18n must be used inside I18nProvider");
  return value;
}

/* ------------------------------ coverage -------------------------------- */

export interface LocaleCoverage {
  translated: number;
  total: number;
  percent: number;
}

type Coverage = Partial<Record<LocaleCode, LocaleCoverage>>;

export function coverageFor(code: LocaleCode): LocaleCoverage | undefined {
  return activeCoverage[code];
}

export function statusLabel(locale: LocaleMeta): string {
  if (locale.status === "official") return t("Official");
  return locale.reviewed ? t("Community · reviewed") : t("Community · unreviewed");
}
