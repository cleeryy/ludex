import { describe, expect, it } from "vitest";
import { consoleEntryRedirect } from "@/lib/console-entry";

describe("console entry rule", () => {
  it("sends a console device to /console instead of the desktop login", () => {
    const active = { active: true, pending: false };
    expect(consoleEntryRedirect("/", active)).toBe("/console");
    expect(consoleEntryRedirect("/login", active)).toBe("/console");
    expect(consoleEntryRedirect("/library", active)).toBeNull();
    expect(consoleEntryRedirect("/console/settings", active)).toBeNull();
  });

  it("leaves a desktop device on its own home", () => {
    const desktop = { active: false, pending: false };
    expect(consoleEntryRedirect("/", desktop)).toBeNull();
    expect(consoleEntryRedirect("/login", desktop)).toBeNull();
  });
});
