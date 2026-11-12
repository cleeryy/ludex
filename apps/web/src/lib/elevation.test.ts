import { describe, expect, it } from "vitest";
import {
  ELEVATION_OPERATIONS,
  configFolderFromAppsPath,
  elevationOperationLabel,
  isElevationCancelled,
} from "./elevation";

/** #192: the elevation dialog explains the exact operation and target. */
describe("elevation helpers", () => {
  it("labels every whitelisted operation", () => {
    for (const operation of ELEVATION_OPERATIONS) {
      expect(elevationOperationLabel(operation)).toBeTruthy();
    }
  });

  it("derives the config folder from a detected apps.json", () => {
    expect(configFolderFromAppsPath("C:\\Program Files\\Sunshine\\config\\apps.json")).toBe(
      "C:\\Program Files\\Sunshine\\config",
    );
    expect(configFolderFromAppsPath("/home/me/.config/sunshine/apps.json")).toBe(
      "/home/me/.config/sunshine",
    );
    expect(configFolderFromAppsPath("C:\\Program Files\\Sunshine\\config\\")).toBe(
      "C:\\Program Files\\Sunshine",
    );
    expect(configFolderFromAppsPath("apps.json")).toBeNull();
    expect(configFolderFromAppsPath(null)).toBeNull();
    expect(configFolderFromAppsPath(undefined)).toBeNull();
  });

  it("only treats UAC cancellation as cancelled", () => {
    expect(isElevationCancelled("cancelled")).toBe(true);
    expect(isElevationCancelled("granted")).toBe(false);
    expect(isElevationCancelled("failed")).toBe(false);
    expect(isElevationCancelled(null)).toBe(false);
  });
});
