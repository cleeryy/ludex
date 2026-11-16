import fs from "node:fs";
import { describe, expect, it } from "vitest";
import { buildAppearance } from "@/theme/storage";
import { builtinTheme } from "@/theme/registry";

/**
 * No-flash boot guard (#140). The inline script in index.html runs while the
 * document is parsing, before the body paints. These tests execute the real
 * shipped script against a fake document and assert it replays the stored
 * variable map synchronously.
 */

interface InlineScript {
  attributes: string;
  body: string;
}

/** Index of the `>` that closes a tag, ignoring `>` inside quoted values. */
function tagEnd(html: string, from: number): number {
  let quote: string | null = null;
  for (let index = from; index < html.length; index += 1) {
    const character = html[index];
    if (quote) {
      if (character === quote) quote = null;
    } else if (character === '"' || character === "'") {
      quote = character;
    } else if (character === ">") {
      return index;
    }
  }
  return -1;
}

/**
 * Every inline `<script>` in a document, attributes included.
 *
 * A scanner instead of a tag regexp: `js/bad-tag-filter` is right that
 * `/<script[^>]*>(.*?)<\/script\s*>/` disagrees with a browser on end tags such
 * as `</script foo>`, and this file is read by the test, not by an HTML parser.
 * Case-insensitive, quote-aware and it rejects `<scripted>`-style names.
 */
function inlineScripts(html: string): InlineScript[] {
  const lower = html.toLowerCase();
  const scripts: InlineScript[] = [];
  let cursor = 0;
  for (;;) {
    const open = lower.indexOf("<script", cursor);
    if (open === -1) break;
    const nameEnd = open + "<script".length;
    const next = lower[nameEnd];
    if (next !== undefined && !/[\s/>]/.test(next)) {
      cursor = nameEnd;
      continue;
    }
    const close = tagEnd(html, nameEnd);
    if (close === -1) break;
    const end = lower.indexOf("</script", close + 1);
    if (end === -1) break;
    scripts.push({ attributes: html.slice(nameEnd, close), body: html.slice(close + 1, end) });
    cursor = end + "</script".length;
  }
  return scripts;
}

const html = fs.readFileSync(new URL("../../index.html", import.meta.url), "utf8");
// The boot guard is the script without attributes that carries the appearance
// replay (`ludex-appearance`).
const boot = inlineScripts(html).find(({ attributes, body }) => attributes.trim() === "" && body.trim() !== "");
if (!boot) throw new Error("index.html is missing the inline boot script");
const script = boot.body;

describe("inline script extraction", () => {
  it("is case-insensitive, attribute-aware and tolerant of end-tag noise", () => {
    const sample = '<SCRIPT type="module">external()</SCRIPT><script>boot()</script\t\n data-x>';
    expect(inlineScripts(sample)).toEqual([
      { attributes: ' type="module"', body: "external()" },
      { attributes: "", body: "boot()" },
    ]);
    // `<scripted>` is not a script element.
    expect(inlineScripts('<scripted src="x"></scripted>')).toEqual([]);
    // `>` inside an attribute value does not close the opening tag.
    expect(inlineScripts('<script data-value="a>b">run()</script>')).toEqual([
      { attributes: ' data-value="a>b"', body: "run()" },
    ]);
  });
});

interface BootResult {
  dataset: Record<string, string>;
  vars: Map<string, string>;
  colorScheme: string;
  meta: string;
  lang: string;
  dir: string;
}

function runBoot(stored: string | null, light: boolean, locale = "en"): BootResult {
  const vars = new Map<string, string>();
  const dataset: Record<string, string> = {};
  const root = {
    dataset,
    lang: "",
    dir: "",
    style: {
      colorScheme: "",
      setProperty(key: string, value: string) {
        vars.set(key, value);
      },
    },
  };
  let meta = "";
  const document = {
    documentElement: root,
    querySelector: () => ({
      setAttribute: (_name: string, value: string) => {
        meta = value;
      },
    }),
  };
  const localStorage = {
    getItem: (key: string) => {
      if (key === "ludex.theme") return stored;
      if (key === "ludex.locale") return locale;
      return null;
    },
  };
  const window = { matchMedia: () => ({ matches: light }) };
  new Function("localStorage", "window", "document", script)(localStorage, window, document);
  return { dataset, vars, colorScheme: root.style.colorScheme, meta, lang: root.lang, dir: root.dir };
}

describe("theme boot script", () => {
  const nord = buildAppearance({ selection: "nord", accent: "#88c0d0", tint: null });

  it("applies a fixed theme, its accent and the browser chrome colour", () => {
    const boot = runBoot(JSON.stringify(nord), false);
    expect(boot.dataset.theme).toBe("nord");
    expect(boot.dataset.themeMode).toBe("dark");
    expect(boot.dataset.themeSelection).toBe("nord");
    expect(boot.dataset.tv).toBe("false");
    expect(boot.vars.get("--bg")).toBe(builtinTheme("nord").tokens.bg);
    expect(boot.vars.get("--signal")).toBe("#88c0d0");
    expect(boot.colorScheme).toBe("dark");
    expect(boot.meta).toBe(nord.themeColor.dark);
  });

  it("follows the OS for the system option", () => {
    const system = buildAppearance({ selection: "system", accent: null, tint: null });
    const dark = runBoot(JSON.stringify(system), false);
    expect(dark.dataset.theme).toBe("midnight");
    expect(dark.dataset.themeMode).toBe("dark");
    expect(dark.vars.get("--bg")).toBe(builtinTheme("midnight").tokens.bg);

    const light = runBoot(JSON.stringify(system), true);
    expect(light.dataset.theme).toBe("light");
    expect(light.dataset.themeMode).toBe("light");
    expect(light.vars.get("--bg")).toBe(builtinTheme("light").tokens.bg);
    expect(light.meta).toBe(system.themeColor.light);
  });

  it("migrates the legacy dark/light string without a stored map", () => {
    const legacy = runBoot("light", false);
    expect(legacy.dataset.theme).toBe("light");
    expect(legacy.dataset.themeMode).toBe("light");
    expect(legacy.vars.size).toBe(0);
    expect(legacy.meta).toBe("#e7e2d4");
  });

  it("survives corrupt storage and still picks the OS mode", () => {
    const boot = runBoot("{not json", true);
    expect(boot.dataset.theme).toBe("light");
    expect(boot.dataset.themeMode).toBe("light");
    expect(boot.vars.size).toBe(0);
  });

  it("keeps the stored locale and direction in the same pass", () => {
    const boot = runBoot(null, false, "ar");
    expect(boot.lang).toBe("ar");
    expect(boot.dir).toBe("rtl");
  });

  it("runs synchronously, with no deferred work", () => {
    expect(script).not.toMatch(/\b(setTimeout|requestAnimationFrame|requestIdleCallback|Promise|async|await)\b/);
  });
});
