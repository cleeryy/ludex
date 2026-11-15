import { describe, expect, it } from "vitest";
import {
  GLYPH_THEMES,
  familyFromPadId,
  glyphFamilyFor,
  identityLabel,
  normalizePadName,
  padFamilyLabel,
  padFingerprint,
  shortFingerprint,
} from "./controller-identity";

/* --------------------------------------------------------------------------
   #230: identity has to survive a reconnect, a reboot, another USB port and a
   second identical pad. `gamepad.index` is connection order and never appears
   in a fingerprint, so these tests only vary the id string.
   -------------------------------------------------------------------------- */

const CHROME_XBOX =
  "Xbox Wireless Controller (STANDARD GAMEPAD Vendor: 045e Product: 0b13)";
const CHROME_DUALSENSE =
  "DualSense Wireless Controller (STANDARD GAMEPAD Vendor: 054c Product: 0ce6)";
const FIREFOX_XBOX = "xbox wireless controller";

describe("pad fingerprint", () => {
  it("parses the vendor/product pair Chromium appends to the id", () => {
    const pad = padFingerprint(CHROME_XBOX);
    expect(pad.vendor).toBe("045e");
    expect(pad.product).toBe("0b13");
    expect(pad.fingerprint).toBe("usb:045e:0b13");
    expect(pad.standard).toBe(true);
    expect(pad.virtual).toBe(false);
    expect(pad.family).toBe("xbox");
    expect(pad.rawId).toBe(CHROME_XBOX);
  });

  it("lower-cases vendor/product, so a case difference does not split a pad", () => {
    const lower = padFingerprint("Pad (STANDARD GAMEPAD Vendor: 045e Product: 0b13)");
    const upper = padFingerprint("Pad (STANDARD GAMEPAD Vendor: 045E Product: 0B13)");
    expect(upper.fingerprint).toBe(lower.fingerprint);
    expect(upper.vendor).toBe("045e");
    expect(upper.product).toBe("0b13");
  });

  it("returns the same fingerprint for the same id, twice and after a reconnect", () => {
    const first = padFingerprint(CHROME_DUALSENSE);
    const again = padFingerprint(CHROME_DUALSENSE);
    expect(again.fingerprint).toBe(first.fingerprint);
  });

  it("shares one fingerprint between two identical pads, by design", () => {
    // Two DualSense pads on one couch: the Gamepad API cannot tell them apart
    // across sessions, so they share a profile.
    const left = padFingerprint(CHROME_DUALSENSE);
    const right = padFingerprint(CHROME_DUALSENSE);
    expect(left.fingerprint).toBe(right.fingerprint);
  });

  it("hashes the normalised name when Firefox/Safari omit vendor and product", () => {
    const pad = padFingerprint(FIREFOX_XBOX);
    expect(pad.vendor).toBeNull();
    expect(pad.product).toBeNull();
    expect(pad.fingerprint).toMatch(/^name:[0-9a-f]{8}$/);
    // Same pad, different capitalisation and spacing: one fingerprint.
    expect(padFingerprint("Xbox  Wireless Controller").fingerprint).toBe(pad.fingerprint);
    // Different pad: different fingerprint.
    expect(padFingerprint("DualSense Wireless Controller").fingerprint).not.toBe(pad.fingerprint);
  });

  it("does not carry the connection index into the fingerprint", () => {
    // The index is deliberately absent from the signature; two calls that would
    // differ only by connection order are the same identity.
    expect(padFingerprint(CHROME_XBOX).fingerprint).toBe(padFingerprint(CHROME_XBOX).fingerprint);
    expect(padFingerprint(CHROME_XBOX).fingerprint).not.toContain("index");
  });

  it("uses the live mapping when the browser provides one", () => {
    expect(padFingerprint("USB Gamepad", undefined, { mapping: "standard" }).standard).toBe(true);
    expect(padFingerprint(CHROME_XBOX, undefined, { mapping: "" }).standard).toBe(false);
  });

  it("flags Steam Input virtual pads and maps them as Xbox", () => {
    const pad = padFingerprint(
      "Steam Virtual Gamepad (STANDARD GAMEPAD Vendor: 28de Product: 11ff)",
    );
    expect(pad.virtual).toBe(true);
    expect(pad.family).toBe("xbox");
    expect(identityLabel(pad)).toBe("Steam Input (virtual)");
  });

  it("flags an XInput pad with no vendor block as virtual", () => {
    const pad = padFingerprint("Xbox 360 Controller (XInput STANDARD GAMEPAD)");
    expect(pad.virtual).toBe(true);
    expect(pad.family).toBe("xbox");
  });

  it("keeps a physical pad non-virtual when the id carries a vendor block", () => {
    const pad = padFingerprint("Xbox 360 Controller (XInput STANDARD GAMEPAD Vendor: 045e Product: 028e)");
    expect(pad.virtual).toBe(false);
    expect(identityLabel(pad)).toBe("Xbox");
  });

  it("keeps a real Steam Deck controller out of the virtual bucket", () => {
    const pad = padFingerprint("Steam Deck Controller (STANDARD GAMEPAD Vendor: 28de Product: 1205)");
    expect(pad.family).toBe("steamdeck");
    expect(pad.virtual).toBe(false);
    expect(identityLabel(pad)).toBe("Steam Deck");
  });

  it("falls back to the family the caller guessed, then to generic", () => {
    expect(padFingerprint("USB Gamepad", "dualsense").family).toBe("dualsense");
    expect(padFingerprint("USB Gamepad").family).toBe("generic");
    expect(padFingerprint(null).family).toBe("generic");
    expect(padFingerprint(undefined).fingerprint).toMatch(/^name:/);
  });
});

describe("pad names", () => {
  it("strips the product block, the mapping name and the punctuation", () => {
    expect(normalizePadName(CHROME_XBOX)).toBe("xbox wireless controller");
    expect(normalizePadName("  Xbox—Wireless/Controller  ")).toBe("xbox wireless controller");
    expect(normalizePadName("Vendor: 045e Product: 0b13")).toBe("");
  });

  it("labels the glyph families", () => {
    expect(padFamilyLabel("switch")).toBe("Switch");
    expect(padFamilyLabel("numbers")).toBe("Numbers");
    expect(padFamilyLabel("generic")).toBe("Generic");
  });

  it("detects the Switch and Steam families", () => {
    expect(familyFromPadId("Pro Controller (STANDARD GAMEPAD Vendor: 057e Product: 2009)")).toBe("switch");
    expect(familyFromPadId("Nintendo Switch Pro Controller")).toBe("switch");
    expect(familyFromPadId("Steam Virtual Gamepad")).toBe("xbox");
    expect(familyFromPadId("USB Gamepad")).toBeNull();
  });

  it("shortens a fingerprint for a table cell", () => {
    expect(shortFingerprint("usb:045e:0b13")).toBe("045e:0b13");
    expect(shortFingerprint("name:1a2b3c4d")).toBe("1a2b3c4d");
  });
});

describe("glyph themes", () => {
  it("covers every theme and follows the pad on automatic", () => {
    const pad = padFingerprint(CHROME_DUALSENSE);
    expect(GLYPH_THEMES).toEqual(["auto", "xbox", "playstation", "switch", "steamdeck", "numbers"]);
    expect(glyphFamilyFor("auto", pad)).toBe("dualsense");
    expect(glyphFamilyFor("xbox", pad)).toBe("xbox");
    expect(glyphFamilyFor("playstation", pad)).toBe("dualsense");
    expect(glyphFamilyFor("switch", pad)).toBe("switch");
    expect(glyphFamilyFor("steamdeck", pad)).toBe("steamdeck");
    expect(glyphFamilyFor("numbers", pad)).toBe("numbers");
  });
});
