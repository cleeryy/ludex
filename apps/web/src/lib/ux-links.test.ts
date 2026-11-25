import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { SETTINGS_SECTIONS } from "./settings-sections";

const WEB_SRC = resolve(import.meta.dirname, "..");
const read = (rel: string) => readFileSync(resolve(WEB_SRC, rel), "utf8");
const VALID_SECTIONS = new Set<string>(SETTINGS_SECTIONS.map((section) => section.id));

describe("UX audit invariants (#354)", () => {
  it("forwards active library filters from the topbar random-game action", () => {
    const shell = read("components/shell/app-shell.tsx");
    expect(shell).toContain('const onLibrary = location.pathname === "/library"');
    expect(shell).toContain('filterQuery(readFilters(params, "library"))');
    expect(shell).toContain('t("Your profile")');
    expect(shell).toContain('t("Keyboard shortcuts")');
  });

  it("deep-links to valid settings sections from community, friends, profile, remote, and home", () => {
    const targets: Array<[string, string]> = [
      ["screens/community.tsx", "/settings?section=account"],
      ["screens/friends.tsx", "/settings?section=account"],
      ["screens/profile.tsx", "/settings?section=account"],
      ["screens/remote.tsx", "/settings?section=integrations"],
      ["screens/home.tsx", "/settings?section=storage"],
    ];

    for (const [file, expectedLink] of targets) {
      const source = read(file);
      expect(source).toContain(`to="${expectedLink}"`);
      const section = new URLSearchParams(expectedLink.split("?")[1]).get("section");
      expect(section && VALID_SECTIONS.has(section)).toBe(true);
    }

    // No bare `/settings` links left on community, friends, profile, or remote.
    for (const file of ["screens/community.tsx", "screens/friends.tsx", "screens/profile.tsx", "screens/remote.tsx"]) {
      expect(read(file)).not.toContain('to="/settings"');
    }
  });

  it("gates destructive friend removal behind ConfirmDialog on friends and profile screens", () => {
    for (const file of ["screens/friends.tsx", "screens/profile.tsx"]) {
      const source = read(file);
      expect(source).toContain("<ConfirmDialog");
      expect(source).toContain('t("Remove {name} from friends?"');
    }
  });

  it("keeps the sort selector visible in the library toolbar on desktop", () => {
    const library = read("screens/library.tsx");
    expect(library).not.toContain('w-[150px] md:hidden');
  });
});
