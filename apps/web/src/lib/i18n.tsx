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
import enCatalog from "../locales/en.json";
import coverageData from "../locales/coverage.json";
import localeManifest from "../locales/manifest.json";
import { PSEUDO_LOCALE, pseudoCatalog } from "./pseudo";

/**
 * Ludex i18n. English is the source language, so the English message id *is*
 * the copy ("gettext" style). Locale files are flat JSON maps keyed by that
 * message id; a missing entry falls back to English. Plurals use a `|category`
 * suffix on the message id (`Intl.PluralRules` categories), so a translator can
 * add `|one`, `|few`, `|many`, ... without touching the code.
 *
 * `apps/web/scripts/i18n.mjs` owns the catalog: it extracts every `t("…")` and
 * `tn(…, "one", "other")` from the source, writes `src/locales/en.json`, checks
 * key parity, placeholders, plural categories, RTL coherence, coverage
 * thresholds and the pseudo-locale, and keeps `coverage.json` fresh.
 *
 * Locale selection rules (#139):
 *   - a locale is offered in the picker when its catalog covers at least
 *     `SELECTABLE_PERCENT` of the ids (official locales are complete by CI);
 *   - everything else stays hidden and the user sees English, key by key —
 *     never a raw key and never a half-translated screen;
 *   - the choice is stored per device; the first run follows the browser
 *     (`navigator.languages`) and `?lang=xx` forces a locale for that session
 *     without persisting it (translator/reviewer escape hatch).
 */

export type LocaleCode = string;
export type LocaleDir = "ltr" | "rtl";
/** `official` = reviewed, complete; `preview` = shipped, still being reviewed. */
export type LocaleStatus = "official" | "preview" | "pseudo";
/** `top10` locales are the #139 target set; `extended` is everything after. */
export type LocaleTier = "source" | "top10" | "extended" | "dev";

export interface LocaleMeta {
  code: LocaleCode;
  /** Native name, shown in the picker. */
  label: string;
  englishLabel: string;
  dir: LocaleDir;
  status: LocaleStatus;
  tier: LocaleTier;
  /** Community translations start unreviewed; Arabic is machine-seeded. */
  reviewed: boolean;
  /** Expected catalog file, relative to `src/locales`. */
  file: string | null;
  /** Intl locale used for dates, numbers, relative times and plurals. */
  intl: string;
}

export const LOCALE_STORAGE_KEY = "ludex.locale";
export const DEFAULT_LOCALE: LocaleCode = "en";
/** Below this coverage a locale is hidden from the picker. */
export const SELECTABLE_PERCENT = 95;

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
 * TypeScript changes are needed. `en-XA` is generated from the English catalog
 * at runtime and never ships a file.
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

/* ------------------------------ coverage -------------------------------- */

export interface LocaleCoverage {
  translated: number;
  total: number;
  percent: number;
}

type Coverage = Partial<Record<LocaleCode, LocaleCoverage>>;

const activeCoverage: Coverage = coverageData as Coverage;

export function coverageFor(code: LocaleCode): LocaleCoverage | undefined {
  const coverage = activeCoverage[code];
  if (!coverage) {
    return code === DEFAULT_LOCALE ? { translated: 1, total: 1, percent: 100 } : undefined;
  }
  // The source can grow between catalog regenerations; never report > 100 %.
  return { ...coverage, percent: Math.min(100, coverage.percent) };
}

export function coveragePercent(code: LocaleCode): number {
  return coverageFor(code)?.percent ?? 0;
}

/** Whether the picker offers the locale, i.e. the UI would not mix languages. */
export function isSelectable(code: LocaleCode): boolean {
  if (!LOCALE_BY_CODE.has(code)) return false;
  const meta = localeMeta(code);
  if (meta.status === "pseudo") return false;
  if (meta.status === "official") return true;
  return coveragePercent(code) >= SELECTABLE_PERCENT;
}

/** Locales the picker can show, official first then best-covered previews. */
export function selectableLocales(): LocaleMeta[] {
  return LOCALES.filter((locale) => locale.status !== "pseudo")
    .filter((locale) => isSelectable(locale.code))
    .sort((a, b) => {
      if (a.status !== b.status) return a.status === "official" ? -1 : 1;
      return coveragePercent(b.code) - coveragePercent(a.code);
    });
}

/* ------------------------------ selection ------------------------------- */

function storeLocale(code: LocaleCode): void {
  try {
    localStorage.setItem(LOCALE_STORAGE_KEY, code);
  } catch {
    /* private mode: the preference is session-only */
  }
}

/** Stored preference, or `null` on a first run (and in private mode). */
export function storedLocale(): LocaleCode | null {
  try {
    const stored = localStorage.getItem(LOCALE_STORAGE_KEY);
    return isLocaleCode(stored) ? stored : null;
  } catch {
    return null;
  }
}

export function getStoredLocale(): LocaleCode {
  return storedLocale() ?? DEFAULT_LOCALE;
}

/**
 * `?lang=xx` (also accepted inside the hash route) forces a locale for this
 * session without persisting it. Reviewers and translators use it to look at a
 * catalog that the coverage gate still hides.
 */
export function readLocaleOverride(href?: string): LocaleCode | null {
  const haystack =
    href ?? (typeof window === "undefined" ? "" : `${window.location.search}${window.location.hash}`);
  if (!haystack) return null;
  const match = /[?&]lang=([A-Za-z-]{2,16})/.exec(haystack);
  if (!match) return null;
  const wanted = match[1].toLowerCase();
  return LOCALES.find((locale) => locale.code.toLowerCase() === wanted)?.code ?? null;
}

function browserLanguages(): string[] {
  if (typeof navigator === "undefined") return [];
  const languages = navigator.languages?.length ? navigator.languages : [navigator.language];
  return languages.filter((language): language is string => typeof language === "string" && language.length > 0);
}

/**
 * Best selectable locale for an ordered language list, walking the list in the
 * user's priority order: an exact BCP-47 match (`pt-BR`) first, then the
 * best-covered sibling of the base language (`pt` → `pt-BR`, `zh` → `zh-CN`).
 * Locales below the coverage gate are never detected automatically, so a
 * half-translated catalog is never forced on someone who did not ask for it.
 */
export function detectLocale(candidates: readonly string[]): LocaleCode | null {
  const normalized = candidates.map((candidate) => candidate.toLowerCase().replace(/_/g, "-"));
  for (const candidate of normalized) {
    const exact = LOCALES.find((locale) => locale.status !== "pseudo" && locale.code.toLowerCase() === candidate);
    if (exact && isSelectable(exact.code)) return exact.code;
    const base = candidate.split("-")[0];
    const siblings = LOCALES.filter(
      (locale) => locale.status !== "pseudo" && locale.code.split("-")[0].toLowerCase() === base,
    ).sort((a, b) => coveragePercent(b.code) - coveragePercent(a.code));
    if (siblings[0] && isSelectable(siblings[0].code)) return siblings[0].code;
  }
  return null;
}

let activeLocale: LocaleCode = DEFAULT_LOCALE;
let activeMessages: Messages = {};
let hydrationToken = 0;

async function loadMessages(code: LocaleCode): Promise<Messages> {
  if (code === DEFAULT_LOCALE) return {};
  // The pseudo-locale is generated from the English catalog: no file, and any
  // string that stays plain ASCII on screen never went through `t()`.
  if (code === PSEUDO_LOCALE) return pseudoCatalog(enCatalog as Messages);
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
  root.dataset.localeStatus = localeMeta(code).status;
}

/** Loads the persisted locale before the first React render (see main.tsx). */
export async function bootstrapI18n(): Promise<void> {
  const override = readLocaleOverride();
  const stored = storedLocale();
  const detected = stored ? null : detectLocale(browserLanguages());
  const code = override ?? stored ?? detected ?? DEFAULT_LOCALE;
  const messages = await loadMessages(code);
  activeLocale = code;
  activeMessages = messages;
  applyDocumentLocale(code);
  // Persist an OS/browser match once, so later visits keep the user's locale
  // even if the browser languages change. An override stays session-only.
  if (!override && !stored && detected) storeLocale(detected);
}

/** Applied before paint by the inline boot script in index.html. */
export function getLocaleFromStorageUnsafe(): LocaleCode {
  return getStoredLocale();
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
  coverage: LocaleCoverage | undefined;
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
    if (next === activeLocale || !isLocaleCode(next)) return;
    await hydrate(next, setLocaleState);
  }, []);

  const value = useMemo<I18nValue>(
    () => ({
      locale,
      meta: localeMeta(locale),
      dir: localeMeta(locale).dir,
      coverage: coverageFor(locale),
      setLocale,
    }),
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
  storeLocale(code);
  applyDocumentLocale(code);
  set(code);
}

export function useI18n(): I18nValue {
  const value = useContext(I18nContext);
  if (!value) throw new Error("useI18n must be used inside I18nProvider");
  return value;
}

/* ------------------------------ labels ---------------------------------- */

export function statusLabel(locale: LocaleMeta): string {
  if (locale.status === "official") return t("Official");
  return locale.reviewed ? t("Community · reviewed") : t("Community · unreviewed");
}
