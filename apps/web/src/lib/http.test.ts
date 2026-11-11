import { describe, expect, it } from "vitest";
import { ApiError, errorMessage } from "./http";
import { t } from "./i18n";

/**
 * Regression test for #150: Tauri commands reject with a plain string, so
 * `errorMessage()` must return that string instead of the generic fallback.
 * Every toast in the app goes through this helper.
 */
describe("errorMessage", () => {
  it("surfaces a Tauri command rejection string", () => {
    const rejection =
      "Sunshine's apps.json is not valid JSON (expected value at line 1 column 1). Nothing was changed; fix or restore the file first.";
    expect(errorMessage(rejection)).toBe(rejection);
  });

  it("falls back to the generic message for empty or unknown values", () => {
    expect(errorMessage("   ")).toBe(t("Something went wrong"));
    expect(errorMessage(undefined)).toBe(t("Something went wrong"));
    expect(errorMessage(null)).toBe(t("Something went wrong"));
    expect(errorMessage({ message: 42 })).toBe(t("Something went wrong"));
  });

  it("reads Error and ApiError messages", () => {
    expect(errorMessage(new Error("boom"))).toBe("boom");
    expect(errorMessage(new ApiError(500, "internal", "The server failed"))).toBe(
      "The server failed",
    );
  });

  it("accepts serialized errors with a message field", () => {
    expect(errorMessage({ message: "serialized failure" })).toBe("serialized failure");
  });
});
