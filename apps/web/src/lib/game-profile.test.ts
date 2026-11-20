import { describe, expect, it } from "vitest";

import { profileFieldLabel, profileValue, proposalHasChanges } from "./game-profile";

describe("profile diff helpers", () => {
  it("shows an em dash for an absent side", () => {
    expect(profileValue(null)).toBe("—");
    expect(profileValue("")).toBe("—");
    expect(profileValue("umu")).toBe("umu");
  });

  it("labels environment and future keys", () => {
    expect(profileFieldLabel("launch.env.DXVK_HUD")).toBe("ENV DXVK_HUD");
    expect(profileFieldLabel("future.audio")).toBe("Future audio");
    expect(profileFieldLabel("display.width")).toBe("display.width");
  });

  it("knows an empty proposal", () => {
    expect(proposalHasChanges([])).toBe(false);
    expect(proposalHasChanges([{ field: "launch.mode", before: "auto", after: "umu" }])).toBe(true);
  });
});
