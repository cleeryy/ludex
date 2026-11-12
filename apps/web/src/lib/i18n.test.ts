import { describe, expect, it } from "vitest";
import {
  cacheAccountLocale,
  cacheServerLocale,
  cachedAccountLocale,
  cachedServerLocale,
  coverageFor,
  coveragePercent,
  detectLocale,
  formatDateValue,
  formatNumber,
  getIntlLocale,
  isRtl,
  isSelectable,
  LOCALE_CODES,
  localeMeta,
  localePickerEntries,
  LOCALES,
  readLocaleOverride,
  resolveLocale,
  selectableLocales,
  SELECTABLE_PERCENT,
  statusLabel,
  t,
  tn,
} from "./i18n";
import { PSEUDO_LOCALE, pseudoCatalog, pseudoText } from "./pseudo";

describe("i18n runtime", () => {
  it("falls back to the English message id", () => {
    expect(t("Home")).toBe("Home");
    expect(t("Try again")).toBe("Try again");
  });

  it("interpolates numbers with the active locale", () => {
    expect(t("{count} games", { count: 3 })).toBe("3 games");
  });

  it("handles singular and plural forms", () => {
    expect(tn(1, "1 game", "{count} games")).toBe("1 game");
    expect(tn(4, "1 game", "{count} games")).toBe("4 games");
  });

  it("formats numbers and dates with Intl", () => {
    expect(formatNumber(1234.5, {}, "fr").replace(/\s/g, " ")).toBe("1 234,5");
    expect(formatDateValue("2026-01-02", { day: "numeric", month: "long", year: "numeric" }, "fr")).toBe(
      "2 janvier 2026",
    );
    expect(getIntlLocale("ar")).toBe("ar");
  });
});

describe("locale registry", () => {
  it("registers the top-10 target set from #139", () => {
    for (const code of ["en", "fr", "zh-CN", "ru", "es", "pt-BR", "de", "ja", "ko", "pl", "tr"]) {
      expect(LOCALE_CODES).toContain(code);
    }
  });

  it("has one official source and keeps French official", () => {
    expect(LOCALES.filter((locale) => locale.status === "official").map((locale) => locale.code)).toEqual(["en", "fr"]);
    expect(localeMeta("en").tier).toBe("source");
    expect(localeMeta("fr").reviewed).toBe(true);
  });

  it("declares a native name, an English name and an expected file per locale", () => {
    for (const locale of LOCALES) {
      expect(locale.label.length).toBeGreaterThan(0);
      expect(locale.englishLabel.length).toBeGreaterThan(0);
      if (locale.status === "pseudo") expect(locale.file).toBeNull();
      else expect(locale.file).toBe(`${locale.code}.json`);
    }
    expect(localeMeta("de").label).toBe("Deutsch");
    expect(localeMeta("es").label).toBe("Español");
    expect(localeMeta("pt-BR").label).toBe("Português (Brasil)");
  });

  it("marks Arabic (and only Arabic here) as RTL", () => {
    expect(LOCALES.filter((locale) => locale.dir === "rtl").map((locale) => locale.code)).toEqual(["ar"]);
    expect(isRtl("ar")).toBe(true);
    expect(isRtl("fr")).toBe(false);
  });
});

describe("coverage gate", () => {
  it("always offers the source and official locales", () => {
    expect(isSelectable("en")).toBe(true);
    expect(isSelectable("fr")).toBe(true);
    expect(coveragePercent("en")).toBe(100);
  });

  it("hides a locale below the 95 % floor", () => {
    expect(coveragePercent("ar")).toBeLessThan(SELECTABLE_PERCENT);
    expect(isSelectable("ar")).toBe(false);
    expect(selectableLocales().map((locale) => locale.code)).not.toContain("ar");
  });

  it("only offers preview locales that clear the floor", () => {
    // Coverage moves while the translation lanes land, so assert the invariant
    // instead of one locale's number.
    for (const locale of selectableLocales()) {
      if (locale.status === "official") continue;
      expect(coveragePercent(locale.code)).toBeGreaterThanOrEqual(SELECTABLE_PERCENT);
    }
  });

  it("never offers the pseudo-locale or an unregistered code", () => {
    expect(isSelectable(PSEUDO_LOCALE)).toBe(false);
    expect(selectableLocales().map((locale) => locale.code)).not.toContain(PSEUDO_LOCALE);
    expect(isSelectable("zz")).toBe(false);
  });

  it("keeps every offered locale in the registry", () => {
    for (const locale of selectableLocales()) {
      expect(LOCALE_CODES).toContain(locale.code);
      expect(isSelectable(locale.code)).toBe(true);
    }
  });

  it("reports generated coverage and nothing for unknown codes", () => {
    const fr = coverageFor("fr");
    expect(fr).toBeDefined();
    expect(fr!.total).toBeGreaterThan(1000);
    expect(fr!.translated).toBeLessThanOrEqual(fr!.total);
    expect(coverageFor("zz")).toBeUndefined();
  });
});

describe("locale detection", () => {
  it("matches an exact BCP-47 tag, then the base language", () => {
    expect(detectLocale(["fr"])).toBe("fr");
    expect(detectLocale(["en-US"])).toBe("en");
    expect(detectLocale(["fr-CA"])).toBe("fr");
  });

  it("prefers the first language in the browser's priority order", () => {
    // `pt-BR` is only auto-selected while it clears the gate; otherwise the
    // next preferred language wins instead of a half-translated catalog.
    expect(detectLocale(["pt", "fr"])).toBe(isSelectable("pt-BR") ? "pt-BR" : "fr");
    expect(detectLocale(["xx", "fr"])).toBe("fr");
    expect(detectLocale([])).toBeNull();
    expect(detectLocale(["xx-YY"])).toBeNull();
  });

  it("never auto-selects a locale under the gate", () => {
    expect(detectLocale(["ar"])).toBeNull();
    expect(detectLocale(["ar-SA", "fr"])).toBe("fr");
  });
});

describe("?lang= override", () => {
  it("accepts a query string or a hash query", () => {
    expect(readLocaleOverride("?lang=de")).toBe("de");
    expect(readLocaleOverride("#/settings?lang=pt-BR")).toBe("pt-BR");
    expect(readLocaleOverride("?foo=1&lang=zh-CN")).toBe("zh-CN");
  });

  it("canonicalises the casing and ignores unknown codes", () => {
    expect(readLocaleOverride("?lang=PT-br")).toBe("pt-BR");
    expect(readLocaleOverride("?lang=zz")).toBeNull();
    expect(readLocaleOverride("")).toBeNull();
  });

  it("opens a locale under the coverage gate on purpose", () => {
    expect(readLocaleOverride("?lang=ar")).toBe("ar");
  });
});

describe("pseudo-locale", () => {
  it("brackets and accents the English copy", () => {
    expect(pseudoText("Library")).toBe("⟦Ŀïƀŕåŕý⟧");
    expect(pseudoText("Save changes")).toBe("⟦Šåṽé çĥåñĝéš⟧");
  });

  it("keeps placeholders intact and expands the text", () => {
    const pseudo = pseudoText("{count} games");
    expect(pseudo).toContain("{count}");
    expect(pseudo.length).toBeGreaterThan("{count} games".length);
  });

  it("generates a catalog for every English message", () => {
    const catalog = { Home: "Home", "{count} games": "{count} games" };
    const generated = pseudoCatalog(catalog);
    expect(Object.keys(generated)).toEqual(Object.keys(catalog));
    for (const value of Object.values(generated)) {
      expect(value.startsWith("⟦")).toBe(true);
      expect(value.endsWith("⟧")).toBe(true);
    }
  });
});

describe("labels", () => {
  it("labels official and preview locales", () => {
    expect(statusLabel(localeMeta("fr"))).toBe("Official");
    expect(statusLabel(localeMeta("de"))).toBe("Community · unreviewed");
  });
});

describe("effective locale resolution (#188)", () => {
  it("follows device → account → server → browser → English", () => {
    expect(
      resolveLocale({ device: "en", account: "fr", server: "de", browser: ["es"] }),
    ).toEqual({ locale: "en", source: "device" });
    expect(resolveLocale({ account: "fr", server: "de", browser: ["es"] })).toEqual({
      locale: "fr",
      source: "account",
    });
    expect(resolveLocale({ server: "de", browser: ["es"] })).toEqual({
      locale: "de",
      source: "server",
    });
    expect(resolveLocale({ browser: ["de-DE", "fr"] })).toEqual({
      locale: "de",
      source: "browser",
    });
    expect(resolveLocale({ browser: [] })).toEqual({ locale: "en", source: "default" });
  });

  it("keeps ?lang= above every stored preference", () => {
    expect(
      resolveLocale({ override: "ja", device: "en", account: "fr", server: "de" }),
    ).toEqual({ locale: "ja", source: "override" });
  });

  it("ignores stored values under the coverage gate", () => {
    // Arabic is registered but cannot be selected, so it never wins.
    expect(resolveLocale({ device: "ar", account: "fr" })).toEqual({
      locale: "fr",
      source: "account",
    });
    expect(resolveLocale({ account: "ar", server: "de" })).toEqual({
      locale: "de",
      source: "server",
    });
  });

  it("canonicalises casing and ignores unknown codes", () => {
    expect(resolveLocale({ account: "PT-br" })).toEqual({ locale: "pt-BR", source: "account" });
    expect(resolveLocale({ device: "zz", account: "de" })).toEqual({
      locale: "de",
      source: "account",
    });
  });
});

describe("language picker state (#188)", () => {
  it("lists every registered locale, including the ones under the gate", () => {
    const entries = localePickerEntries("en");
    const codes = entries.map((entry) => entry.meta.code);
    expect(codes).toContain("ar");
    expect(codes).toContain("nl");
    expect(codes).not.toContain(PSEUDO_LOCALE);
  });

  it("marks below-the-gate locales disabled with a coverage reason", () => {
    const arabic = localePickerEntries("en").find((entry) => entry.meta.code === "ar");
    expect(arabic?.disabled).toBe(true);
    // The percentage follows the generated coverage file; assert the reason
    // shape instead of a number that every new message id changes.
    expect(arabic?.reason).toContain("Below the coverage gate");
    expect(arabic?.reason).toContain("%");
    const french = localePickerEntries("en").find((entry) => entry.meta.code === "fr");
    expect(french?.disabled).toBe(false);
    expect(french?.reason).toBeNull();
  });

  it("marks the effective locale selected", () => {
    const selected = localePickerEntries("de").filter((entry) => entry.selected);
    expect(selected.map((entry) => entry.meta.code)).toEqual(["de"]);
  });
});

describe("locale preference caches (#188)", () => {
  it("round-trips the account and server caches and rejects gated codes", () => {
    const store = new Map<string, string>();
    Object.defineProperty(globalThis, "localStorage", {
      configurable: true,
      value: {
        getItem: (key: string) => store.get(key) ?? null,
        setItem: (key: string, value: string) => store.set(key, value),
        removeItem: (key: string) => store.delete(key),
      },
    });
    try {
      cacheAccountLocale("PT-br");
      expect(cachedAccountLocale()).toBe("pt-BR");
      cacheAccountLocale("ar");
      expect(cachedAccountLocale()).toBeNull();
      cacheServerLocale("fr");
      expect(cachedServerLocale()).toBe("fr");
      cacheServerLocale("");
      expect(cachedServerLocale()).toBeNull();
    } finally {
      Reflect.deleteProperty(globalThis, "localStorage");
    }
  });
});
