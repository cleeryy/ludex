import { describe, expect, it } from "vitest";
import type { CommonFieldsReport, CommonFieldMatch } from "./types";
import {
  canKeepInSync,
  candidateLabel,
  confidenceTone,
  isResolvedField,
  mappingFor,
  resolvableFields,
  targetForField,
} from "./config-fields";

/**
 * Fixtures mirror the camelCase report the Rust command returns (#148): the
 * mock/dev UI and the tested helpers see the same shape as the desktop app.
 */

function match(patch: Partial<CommonFieldMatch> = {}): CommonFieldMatch {
  return {
    field: "language",
    label: "Language",
    confidence: "certain",
    source: "builtin",
    file: "Saved/Config/Windows/Engine.ini",
    key: "Internationalization/Culture",
    value: "fr",
    valueType: "string",
    allowedValues: [],
    candidates: [],
    note: null,
    ...patch,
  };
}

const report: CommonFieldsReport = {
  gameId: 7,
  installPath: "/games/example",
  fields: [
    match(),
    match({
      field: "playerName",
      label: "Player name",
      file: "steam_emu.ini",
      key: "Settings/UserName",
      value: "cleeryy",
    }),
  ],
  mappings: [],
  notes: [],
};

describe("common fields panel (#148)", () => {
  it("resolves certain and likely matches but never an ambiguous one", () => {
    expect(isResolvedField(match())).toBe(true);
    expect(isResolvedField(match({ confidence: "likely" }))).toBe(true);
    const ambiguous = match({
      confidence: "ambiguous",
      file: null,
      key: null,
      value: null,
      valueType: null,
      candidates: [
        {
          file: "steam_emu.ini",
          key: "Settings/Language",
          value: "english",
          valueType: "string",
          line: 2,
          section: "Settings",
          exact: true,
        },
        {
          file: "RUNE/settings.ini",
          key: "Settings/Language",
          value: "english",
          valueType: "string",
          line: 2,
          section: "Settings",
          exact: true,
        },
      ],
    });
    expect(isResolvedField(ambiguous)).toBe(false);
    expect(resolvableFields({ ...report, fields: [ambiguous] })).toEqual([]);
    expect(canKeepInSync({ ...report, fields: [ambiguous] })).toBe(false);
    expect(candidateLabel(ambiguous.candidates[1])).toBe("RUNE/settings.ini · Settings/Language");
  });

  it("hides the panel when nothing matched", () => {
    const empty: CommonFieldsReport = { ...report, fields: [] };
    expect(resolvableFields(empty)).toEqual([]);
    expect(canKeepInSync(empty)).toBe(false);
  });

  it("maps the app locale and profile name onto the two fields", () => {
    expect(targetForField("language", "fr", "cleeryy")).toEqual({
      field: "language",
      value: "fr",
    });
    expect(targetForField("playerName", "fr", " cleeryy ")).toEqual({
      field: "playerName",
      value: "cleeryy",
    });
    expect(targetForField("playerName", "fr", "  ")).toBeNull();
    expect(targetForField("language", "", "cleeryy")).toBeNull();
  });

  it("keeps a template mapping visible next to a detected field", () => {
    const pinned: CommonFieldsReport = {
      ...report,
      mappings: [
        {
          gameId: 7,
          field: "language",
          file: "RUNE/settings.ini",
          key: "Settings/Language",
          createdAt: "2026-11-11T10:00:00Z",
        },
      ],
    };
    expect(mappingFor(pinned, "language")?.file).toBe("RUNE/settings.ini");
    expect(mappingFor(pinned, "playerName")).toBeNull();
    expect(confidenceTone("certain")).toBe("ok");
    expect(confidenceTone("likely")).toBe("signal");
    expect(confidenceTone("ambiguous")).toBe("danger");
  });
});
