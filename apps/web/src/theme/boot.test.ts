import fs from "node:fs";
import { describe, expect, it } from "vitest";
import { buildAppearance } from "@/theme/storage";
import { builtinTheme } from "@/theme/registry";

/**
 * No-flash boot guard (#140, #333). The inline script in index.html runs while
 * the document is parsing, before the body paints. These tests execute the real
 * shipped script against a fake document and assert it replays the stored
 * variable map, `data-radius` and `data-stripe` synchronously.
 */

interface InlineScript {
  attributes: string;
  body: string;
}

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
    expect(inlineScripts('<scripted src="x"></scripted>')).toEqual([]);
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
  const telemetry = buildAppearance({
    selection: "telemetry",
    accent: "#ff6b4a",
    tint: null,
    info: "#38bdf8",
    radius: "swiss",
    stripe: "off",
  });

  it("applies a fixed theme, its C1/C2 accents, radius, stripe and browser chrome colour", () => {
    const boot = runBoot(JSON.stringify(telemetry), false);
    expect(boot.dataset.theme).toBe("telemetry");
    expect(boot.dataset.themeMode).toBe("dark");
    expect(boot.dataset.themeSelection).toBe("telemetry");
    expect(boot.dataset.radius).toBe("swiss");
    expect(boot.dataset.stripe).toBe("off");
    expect(boot.dataset.tv).toBe("false");
    expect(boot.vars.get("--bg")).toBe(builtinTheme("telemetry").tokens.bg);
    expect(boot.vars.get("--signal")).toBe("#ff6b4a");
    expect(boot.vars.get("--info")).toBe("#38bdf8");
    expect(boot.colorScheme).toBe("dark");
    expect(boot.meta).toBe(telemetry.themeColor.dark);
  });

  it("follows the OS for the system option", () => {
    const system = buildAppearance({ selection: "system", accent: null, tint: null });
    const dark = runBoot(JSON.stringify(system), false);
    expect(dark.dataset.theme).toBe("speranza");
    expect(dark.dataset.themeMode).toBe("dark");
    expect(dark.dataset.radius).toBe("shadcn");
    expect(dark.dataset.stripe).toBe("on");
    expect(dark.vars.get("--bg")).toBe(builtinTheme("speranza").tokens.bg);

    const light = runBoot(JSON.stringify(system), true);
    expect(light.dataset.theme).toBe("bauhaus");
    expect(light.dataset.themeMode).toBe("light");
    expect(light.vars.get("--bg")).toBe(builtinTheme("bauhaus").tokens.bg);
    expect(light.meta).toBe(system.themeColor.light);
  });

  it("migrates legacy dark/light strings without a stored map", () => {
    const legacy = runBoot("light", false);
    expect(legacy.dataset.theme).toBe("bauhaus");
    expect(legacy.dataset.themeMode).toBe("light");
    expect(legacy.dataset.radius).toBe("shadcn");
    expect(legacy.dataset.stripe).toBe("on");
    expect(legacy.vars.size).toBe(0);
    expect(legacy.meta).toBe("#f3efe6");
  });

  it("survives corrupt storage and still picks the OS mode", () => {
    const boot = runBoot("{not json", true);
    expect(boot.dataset.theme).toBe("bauhaus");
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
