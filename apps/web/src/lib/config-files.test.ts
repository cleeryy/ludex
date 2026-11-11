import { describe, expect, it } from "vitest";
import type { ConfigFileInfo } from "./types";
import {
  canLoadConfigFile,
  defaultConfigPath,
  filterConfigFiles,
  readOnlyReason,
} from "./config-files";

function file(patch: Partial<ConfigFileInfo> & { path: string }): ConfigFileInfo {
  return {
    name: patch.path.split("/").pop() ?? patch.path,
    format: "ini",
    sizeBytes: 128,
    entries: 2,
    editable: true,
    note: null,
    ...patch,
  };
}

const files: ConfigFileInfo[] = [
  file({ path: "GameUserSettings.ini" }),
  file({ path: "Cracks/RUNE/steam_emu.ini" }),
  file({ path: "steam_emu.ini" }),
  file({
    path: "achievements.json",
    format: "json",
    editable: false,
    note: "The file looks binary (NUL bytes); Ludex only edits text configs.",
  }),
  file({ path: "huge.ini", editable: false, note: "This file is 2097152 bytes; config files above 1024 KiB are not edited." }),
];

describe("config file list (#147)", () => {
  it("lists every file, including previously excluded crack paths", () => {
    const paths = filterConfigFiles(files, "").map((entry) => entry.path);
    expect(paths).toContain("Cracks/RUNE/steam_emu.ini");
    expect(paths).toContain("steam_emu.ini");
    expect(paths).toHaveLength(files.length);
  });

  it("filters by a case-insensitive path fragment", () => {
    expect(filterConfigFiles(files, "  CRACKS ").map((entry) => entry.path)).toEqual([
      "Cracks/RUNE/steam_emu.ini",
    ]);
    expect(filterConfigFiles(files, "steam_emu").map((entry) => entry.path)).toEqual([
      "Cracks/RUNE/steam_emu.ini",
      "steam_emu.ini",
    ]);
    expect(filterConfigFiles(files, "missing")).toEqual([]);
  });

  it("never queries a read-only entry", () => {
    expect(canLoadConfigFile(files[0])).toBe(true);
    expect(canLoadConfigFile(files[3])).toBe(false);
    expect(canLoadConfigFile(undefined)).toBe(false);
  });

  it("shows the per-file note only for read-only entries", () => {
    expect(readOnlyReason(files[0])).toBeNull();
    expect(readOnlyReason(files[3])).toContain("binary");
    expect(readOnlyReason(file({ path: "readonly.ini", editable: false }))).toBeNull();
  });

  it("selects the first editable file, else the first entry", () => {
    expect(defaultConfigPath(files)).toBe("GameUserSettings.ini");
    expect(defaultConfigPath(files.slice(3))).toBe("achievements.json");
    expect(defaultConfigPath([])).toBe("");
  });
});
