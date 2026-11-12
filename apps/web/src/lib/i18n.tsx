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
 * Locale selection rules (#139, #188):
 *   - a locale can be chosen when its catalog covers at least
 *     `SELECTABLE_PERCENT` of the ids (official locales are complete by CI);
 *   - below the gate it stays visible in the picker but disabled, with the
 *     coverage percentage as the reason; missing strings still fall back to
 *     English key by key — never a raw key and never a half-translated screen;
 *   - the account preference comes from the server, cached for the pre-paint
 *     bootstrap; an explicit per-device override wins over it, and the server
 *     default covers signed-out visitors. `?lang=xx` forces a locale for that
 *     session without persisting it (translator/reviewer escape hatch).
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
/** Cached account preference, used before `/api/me` answers (#188). */
export const ACCOUNT_LOCALE_STORAGE_KEY = "ludex.locale.account";
/** Cached server default, used before `/api/config` answers (#188). */
export const SERVER_LOCALE_STORAGE_KEY = "ludex.locale.server";
export const DEFAULT_LOCALE: LocaleCode = "en";
/** Below this coverage a locale is disabled in the picker (#188). */
export const SELECTABLE_PERCENT = 95;

/**
 * Where the effective language came from (#188). `device` is the explicit
 * per-device override, `account` the preference stored on the server, `server`
 * the administrator default, `browser` the OS/browser languages and `default`
 * English. `override` is the `?lang=` reviewer escape hatch.
 */
export type LocaleSource = "override" | "device" | "account" | "server" | "browser" | "default";

export interface LocaleResolution {
  locale: LocaleCode;
  source: LocaleSource;
}

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

/** Canonical casing for a registered code, or `null` when unknown. */
export function canonicalLocale(value: string | null | undefined): LocaleCode | null {
  if (!value) return null;
  const wanted = value.trim().toLowerCase();
  if (!wanted) return null;
  return LOCALES.find((locale) => locale.code.toLowerCase() === wanted)?.code ?? null;
}

/**
 * One picker row: every registered locale stays visible (#188). Below the gate
 * it is disabled with the coverage reason instead of silently disappearing.
 */
export interface LocalePickerEntry {
  meta: LocaleMeta;
  selected: boolean;
  disabled: boolean;
  reason: string | null;
}

export function localePickerEntries(active: LocaleCode): LocalePickerEntry[] {
  return LOCALES.filter((locale) => locale.status !== "pseudo").map((meta) => {
    const selectable = isSelectable(meta.code);
    return {
      meta,
      selected: meta.code === active,
      disabled: !selectable,
      reason: selectable
        ? null
        : t("Below the coverage gate ({percent}% translated)", {
            percent: coveragePercent(meta.code),
          }),
    };
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

function removeStored(key: string): void {
  try {
    localStorage.removeItem(key);
  } catch {
    /* private mode */
  }
}

function readPreference(key: string): LocaleCode | null {
  try {
    const stored = canonicalLocale(localStorage.getItem(key));
    return stored && isSelectable(stored) ? stored : null;
  } catch {
    return null;
  }
}

function writePreference(key: string, code: LocaleCode | null | undefined): void {
  try {
    const canonical = code ? canonicalLocale(code) : null;
    if (canonical && isSelectable(canonical)) localStorage.setItem(key, canonical);
    else localStorage.removeItem(key);
  } catch {
    /* private mode: the preference is session-only */
  }
}

/** Explicit per-device override, or `null` on a first run. */
export function storedLocale(): LocaleCode | null {
  return readPreference(LOCALE_STORAGE_KEY);
}

/** Account preference cached from the last `/api/me` or login (#188). */
export function cachedAccountLocale(): LocaleCode | null {
  return readPreference(ACCOUNT_LOCALE_STORAGE_KEY);
}

/** Server default cached from the last `/api/config` (#188). */
export function cachedServerLocale(): LocaleCode | null {
  return readPreference(SERVER_LOCALE_STORAGE_KEY);
}

/** Caches the account preference for the pre-paint bootstrap. */
export function cacheAccountLocale(code: string | null | undefined): LocaleCode | null {
  const canonical = code ? canonicalLocale(code) : null;
  const selectable = canonical && isSelectable(canonical) ? canonical : null;
  writePreference(ACCOUNT_LOCALE_STORAGE_KEY, selectable);
  return selectable;
}

/** Caches the server default for the pre-paint bootstrap. */
export function cacheServerLocale(code: string | null | undefined): LocaleCode | null {
  const canonical = code ? canonicalLocale(code) : null;
  const selectable = canonical && isSelectable(canonical) ? canonical : null;
  writePreference(SERVER_LOCALE_STORAGE_KEY, selectable);
  return selectable;
}

export function getStoredLocale(): LocaleCode {
  return storedLocale() ?? DEFAULT_LOCALE;
}

/**
 * Resolution order (#188): explicit device override → account preference →
 * server default → browser languages → English. Every stored value must still
 * be selectable; a locale that fell under the coverage gate is skipped.
 */
export function resolveLocale(input: {
  override?: string | null;
  device?: string | null;
  account?: string | null;
  server?: string | null;
  browser?: readonly string[];
}): LocaleResolution {
  // `?lang=` deliberately opens a gated catalog for reviewers.
  const override = canonicalLocale(input.override);
  if (override) return { locale: override, source: "override" };

  const device = canonicalLocale(input.device);
  if (device && isSelectable(device)) return { locale: device, source: "device" };

  const account = canonicalLocale(input.account);
  if (account && isSelectable(account)) return { locale: account, source: "account" };

  const server = canonicalLocale(input.server);
  if (server && isSelectable(server)) return { locale: server, source: "server" };

  const browser = detectLocale(input.browser ?? []);
  if (browser) return { locale: browser, source: "browser" };

  return { locale: DEFAULT_LOCALE, source: "default" };
}

/** Human label for where the effective language came from. */
export function localeSourceLabel(source: LocaleSource): string {
  switch (source) {
    case "device":
      return t("This device");
    case "account":
      return t("Your account");
    case "server":
      return t("Server default");
    case "override":
      return t("Link override");
    default:
      return t("System");
  }
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
let activeSource: LocaleSource = "default";
let activeMessages: Messages = {};
let hydrationToken = 0;
const localeListeners = new Set<() => void>();

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

function currentResolution(): LocaleResolution {
  return resolveLocale({
    override: readLocaleOverride(),
    device: storedLocale(),
    account: cachedAccountLocale(),
    server: cachedServerLocale(),
    browser: browserLanguages(),
  });
}

function notifyLocaleListeners(): void {
  for (const listener of localeListeners) listener();
}

function commitLocale(code: LocaleCode, source: LocaleSource, messages: Messages): void {
  activeLocale = code;
  activeSource = source;
  activeMessages = messages;
  applyDocumentLocale(code);
  notifyLocaleListeners();
}

async function hydrate(code: LocaleCode, source: LocaleSource): Promise<void> {
  const token = ++hydrationToken;
  const messages = await loadMessages(code);
  if (token !== hydrationToken) return;
  commitLocale(code, source, messages);
}

/**
 * Loads the effective locale before the first React render (see `main.tsx`).
 * The three provider caches are written by the app as soon as `/api/config`,
 * login or `/api/me` answers, so a reload paints the account/server language
 * without a flash and without waiting for the network.
 */
export async function bootstrapI18n(): Promise<void> {
  const resolution = currentResolution();
  const messages = await loadMessages(resolution.locale);
  commitLocale(resolution.locale, resolution.source, messages);
}

/** Applied before paint by the inline boot script in index.html. */
export function getLocaleFromStorageUnsafe(): LocaleCode {
  return getStoredLocale();
}

/** Caches the account preference and re-resolves the effective language. */
export async function applyAccountLocale(code: string | null | undefined): Promise<void> {
  cacheAccountLocale(code);
  await refreshLocale();
}

/**
 * Explicit account choice (#188): stores the preference, drops the per-device
 * override and applies the new language immediately.
 */
export async function chooseAccountLocale(code: string | null | undefined): Promise<void> {
  const cached = cacheAccountLocale(code);
  removeStored(LOCALE_STORAGE_KEY);
  if (cached) await hydrate(cached, "account");
  else await refreshLocale();
}

/**
 * Stores an explicit choice for this device and applies it immediately. The
 * account preference is untouched.
 */
export async function applyDeviceLocale(code: LocaleCode): Promise<void> {
  if (!isSelectable(code)) return;
  storeLocale(code);
  await hydrate(code, "device");
}

/** Drops the device override; the account/server/browser order applies again. */
export async function clearDeviceLocale(): Promise<void> {
  removeStored(LOCALE_STORAGE_KEY);
  await refreshLocale();
}

/** Caches the server default and applies it when nothing outranks it. */
export async function applyServerDefaultLocale(code: string | null | undefined): Promise<void> {
  cacheServerLocale(code);
  await refreshLocale();
}

/** Re-resolves the effective locale from the caches, e.g. after logout. */
export async function refreshLocale(): Promise<void> {
  const resolution = currentResolution();
  if (resolution.locale === activeLocale && resolution.source === activeSource) {
    // The source did not move, but a cached preference may have changed (for
    // example an account locale while a device override is active): let the
    // provider repaint its account/device labels.
    notifyLocaleListeners();
    return;
  }
  await hydrate(resolution.locale, resolution.source);
}

export function getLocaleSource(): LocaleSource {
  return activeSource;
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
  /** Where the effective locale came from (#188). */
  source: LocaleSource;
  meta: LocaleMeta;
  dir: LocaleDir;
  coverage: LocaleCoverage | undefined;
  /** Account preference currently cached, when any. */
  accountLocale: LocaleCode | null;
  /** Explicit per-device override, when any. */
  deviceOverride: LocaleCode | null;
  /** Sets the per-device override and applies it immediately. */
  setLocale: (code: LocaleCode) => Promise<void>;
  /** Applies an account choice: caches it and clears the device override. */
  setAccountLocale: (code: LocaleCode) => Promise<void>;
  /** Drops the device override and falls back to account/server/browser. */
  clearDeviceOverride: () => Promise<void>;
  /** Re-resolves after the caches changed elsewhere (login, logout). */
  refresh: () => Promise<void>;
}

const I18nContext = createContext<I18nValue | null>(null);

export function I18nProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<{ locale: LocaleCode; source: LocaleSource }>(() => ({
    locale: activeLocale,
    source: activeSource,
  }));

  useEffect(() => {
    // bootstrapI18n already ran in main.tsx; this covers tests and HMR and
    // keeps the tree in sync when auth/login changes the caches.
    const sync = () => setState({ locale: activeLocale, source: activeSource });
    localeListeners.add(sync);
    sync();
    return () => {
      localeListeners.delete(sync);
    };
  }, []);

  const setLocale = useCallback(async (next: LocaleCode) => {
    if (!isLocaleCode(next) || !isSelectable(next)) return;
    await applyDeviceLocale(next);
  }, []);

  const setAccountLocale = useCallback(async (next: LocaleCode) => {
    if (!isLocaleCode(next) || !isSelectable(next)) return;
    cacheAccountLocale(next);
    removeStored(LOCALE_STORAGE_KEY);
    await hydrate(next, "account");
  }, []);

  const clearDeviceOverride = useCallback(async () => {
    await clearDeviceLocale();
  }, []);

  const refresh = useCallback(async () => {
    await refreshLocale();
  }, []);

  const value = useMemo<I18nValue>(
    () => ({
      locale: state.locale,
      source: state.source,
      meta: localeMeta(state.locale),
      dir: localeMeta(state.locale).dir,
      coverage: coverageFor(state.locale),
      accountLocale: cachedAccountLocale(),
      deviceOverride: storedLocale(),
      setLocale,
      setAccountLocale,
      clearDeviceOverride,
      refresh,
    }),
    [state, setLocale, setAccountLocale, clearDeviceOverride, refresh],
  );

  return (
    <I18nContext.Provider value={value}>
      {/* Locale changes remount the tree: copy comes from module-level `t`,
          which is not a hook, so a keyed remount is the cheapest correct
          update. Providers above this one keep their state. */}
      <Fragment key={state.locale}>{children}</Fragment>
    </I18nContext.Provider>
  );
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
