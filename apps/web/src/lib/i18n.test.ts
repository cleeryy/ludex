import { describe, expect, it } from "vitest";
import {
  coverageFor,
  formatDateValue,
  formatNumber,
  getIntlLocale,
  LOCALE_CODES,
  localeMeta,
  LOCALES,
  t,
  tn,
} from "./i18n";

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

  it("registers every locale and marks Arabic as RTL", () => {
    expect(LOCALE_CODES).toEqual(["en", "fr", "de", "es", "ar"]);
    expect(localeMeta("ar").dir).toBe("rtl");
    expect(localeMeta("ar").reviewed).toBe(false);
    expect(LOCALES.filter((locale) => locale.status === "official").map((locale) => locale.code)).toEqual(["en", "fr"]);
  });

  it("ships a complete French catalog and seeded community catalogs", () => {
    expect(coverageFor("fr")?.percent).toBe(100);
    for (const code of ["de", "es", "ar"] as const) {
      expect(coverageFor(code)?.percent).toBeGreaterThan(10);
    }
  });
});
