import { describe, expect, it } from "vitest";
import { CONTROLLER_PRESETS, presetProfile } from "./controller-profiles";
import { padFingerprint } from "./controller-identity";
import { createControllerDevicesStore, type ControllerStorage } from "./controller-devices";
import { resolveProfileForPad } from "./controller-resolve";

/* --------------------------------------------------------------------------
   #230: the resolver is the single answer to "which profile drives this pad".
   The order is device entry, then the Console choice, then the per-game
   override, then the account profile, then the family preset — and it never
   returns a profile a player cannot navigate with.
   -------------------------------------------------------------------------- */

const XBOX_ID = "Xbox Wireless Controller (STANDARD GAMEPAD Vendor: 045e Product: 0b13)";
const DUALSENSE_ID = "DualSense Wireless Controller (STANDARD GAMEPAD Vendor: 054c Product: 0ce6)";
const XBOX = padFingerprint(XBOX_ID);

const PRESETS = CONTROLLER_PRESETS.map((id) => ({
  id,
  label: presetProfile(id).name,
  profile: presetProfile(id),
}));

const accountProfile = { ...presetProfile("dualsense"), name: "Account dualsense" };
const gameProfile = { ...presetProfile("steamdeck"), name: "Hades deck" };

function memoryStorage(): ControllerStorage {
  const map = new Map<string, string>();
  return { getItem: (key) => map.get(key) ?? null, setItem: (key, value) => void map.set(key, value) };
}

function freshStore() {
  return createControllerDevicesStore({ storage: memoryStorage(), key: "test.devices", listen: false });
}

describe("profile precedence", () => {
  it("follows the automatic chain when the pad has no entry", () => {
    const result = resolveProfileForPad(XBOX_ID, "xbox", freshStore().get(), {
      accountProfile,
      gameProfile,
      gameId: 7,
      presets: PRESETS,
    });
    expect(result.source).toEqual({ kind: "auto" });
    expect(result.entry).toBeNull();
    expect(result.profile.name).toBe("Hades deck");
  });

  it("uses the Console profile choice before the automatic chain", () => {
    const result = resolveProfileForPad(XBOX_ID, "xbox", freshStore().get(), {
      accountProfile,
      gameProfile,
      gameId: 7,
      presets: PRESETS,
      prefsProfileId: "preset:dualsense",
    });
    expect(result.profile.name).toBe("DualSense");
  });

  it("ignores a per-game Console choice that belongs to another game", () => {
    const result = resolveProfileForPad(XBOX_ID, "xbox", freshStore().get(), {
      accountProfile,
      gameProfile,
      gameId: 7,
      presets: PRESETS,
      prefsProfileId: "game:99",
    });
    // Falls through to the current game's override.
    expect(result.profile.name).toBe("Hades deck");
  });

  it("lets the device entry pick an account profile, even over a game override", () => {
    const store = freshStore();
    const identity = padFingerprint(XBOX_ID);
    store.assign(identity, { source: { kind: "account" } });
    const result = resolveProfileForPad(XBOX_ID, "xbox", store.get(), {
      accountProfile,
      gameProfile,
      gameId: 7,
      presets: PRESETS,
    });
    expect(result.source).toEqual({ kind: "account" });
    expect(result.profile.name).toBe("Account dualsense");
  });

  it("applies a per-game device entry only while that game is open", () => {
    const store = freshStore();
    store.assign(padFingerprint(XBOX_ID), { source: { kind: "game", gameId: 7 } });
    const open = resolveProfileForPad(XBOX_ID, "xbox", store.get(), {
      accountProfile,
      gameProfile,
      gameId: 7,
      presets: PRESETS,
    });
    expect(open.profile.name).toBe("Hades deck");
    const other = resolveProfileForPad(XBOX_ID, "xbox", store.get(), {
      accountProfile,
      gameProfile: null,
      gameId: 9,
      presets: PRESETS,
    });
    // The entry says "game 7" and this screen has no override: account wins.
    expect(other.profile.name).toBe("Account dualsense");
  });

  it("resolves a preset device entry by id", () => {
    const store = freshStore();
    store.assign(padFingerprint(XBOX_ID), { source: { kind: "preset", id: "steamdeck" } });
    const result = resolveProfileForPad(XBOX_ID, "xbox", store.get(), {
      accountProfile,
      gameProfile,
      presets: PRESETS,
    });
    expect(result.source).toEqual({ kind: "preset", id: "steamdeck" });
    expect(result.profile.name).toBe("Steam Deck");
  });

  it("falls back to the family preset when nothing else exists", () => {
    const result = resolveProfileForPad(DUALSENSE_ID, "dualsense", null, {});
    expect(result.profile.name).toBe("DualSense");
    expect(resolveProfileForPad(XBOX_ID, "xbox", null, {}).profile.name).toBe("Xbox");
  });
});

describe("reconnect and identical pads", () => {
  it("keeps the profile across a reconnect, including a reload of the store", () => {
    const storage = memoryStorage();
    const store = createControllerDevicesStore({ storage, key: "test.devices", listen: false });
    store.assign(padFingerprint(XBOX_ID), { source: { kind: "preset", id: "dualsense" } });

    const afterReconnect = resolveProfileForPad(XBOX_ID, "xbox", store.get(), {
      accountProfile,
      presets: PRESETS,
    });
    expect(afterReconnect.profile.name).toBe("DualSense");

    // A new session reads the same device-local document.
    const reloaded = createControllerDevicesStore({ storage, key: "test.devices", listen: false });
    const afterReload = resolveProfileForPad(XBOX_ID, "xbox", reloaded.get(), {
      accountProfile,
      presets: PRESETS,
    });
    expect(afterReload.profile.name).toBe("DualSense");
    expect(afterReload.entry?.identity.fingerprint).toBe(XBOX.fingerprint);
  });

  it("gives two identical pads the same fingerprint and the same profile", () => {
    const store = freshStore();
    store.assign(padFingerprint(XBOX_ID), { source: { kind: "preset", id: "generic" } });
    const first = resolveProfileForPad(XBOX_ID, "xbox", store.get(), { accountProfile, presets: PRESETS });
    const second = resolveProfileForPad(XBOX_ID, "xbox", store.get(), { accountProfile, presets: PRESETS });
    expect(first.identity.fingerprint).toBe(second.identity.fingerprint);
    expect(first.profile.name).toBe(second.profile.name);
    expect(Object.keys(store.get().pads)).toHaveLength(1);
  });

  it("keeps a different pad on its own entry", () => {
    const store = freshStore();
    store.assign(padFingerprint(XBOX_ID), { source: { kind: "preset", id: "generic" } });
    store.assign(padFingerprint(DUALSENSE_ID), { source: { kind: "preset", id: "dualsense" } });
    expect(
      resolveProfileForPad(XBOX_ID, "xbox", store.get(), { presets: PRESETS }).profile.name,
    ).toBe("Generic");
    expect(
      resolveProfileForPad(DUALSENSE_ID, "dualsense", store.get(), { presets: PRESETS }).profile.name,
    ).toBe("DualSense");
  });
});

describe("A/B swap and fallback", () => {
  it("swaps the two face buttons when the device asks for it", () => {
    const store = freshStore();
    const identity = padFingerprint(XBOX_ID);
    store.assign(identity, { source: { kind: "preset", id: "xbox" }, swapAB: true });
    const result = resolveProfileForPad(XBOX_ID, "xbox", store.get(), { presets: PRESETS });
    expect(result.swapAB).toBe(true);
    // Xbox preset: button0 = confirm, button1 = back. Swapped.
    expect(result.profile.mappings.button0).toBe("back");
    expect(result.profile.mappings.button1).toBe("confirm");
  });

  it("does not swap without the toggle", () => {
    const result = resolveProfileForPad(XBOX_ID, "xbox", freshStore().get(), { presets: PRESETS });
    expect(result.swapAB).toBe(false);
    expect(result.profile.mappings.button0).toBe("confirm");
    expect(result.profile.mappings.button1).toBe("back");
  });

  it("synthesises the generic fallback for a non-standard pad", () => {
    const result = resolveProfileForPad(XBOX_ID, "xbox", freshStore().get(), {
      accountProfile,
      presets: PRESETS,
      pad: { mapping: "", buttons: 8, axes: 4 },
    });
    expect(result.fallback).toBe(true);
    expect(result.profile.mappings.button0).toBe("confirm");
    expect(result.profile.mappings["axis0-"]).toBe("left");
  });

  it("leaves a standard pad on its selected profile", () => {
    const result = resolveProfileForPad(XBOX_ID, "xbox", freshStore().get(), {
      accountProfile,
      presets: PRESETS,
      pad: { mapping: "standard", buttons: 16, axes: 4 },
    });
    expect(result.fallback).toBe(false);
    expect(result.profile.name).toBe("Account dualsense");
  });

  it("never returns an unmapped profile", () => {
    const empty = { ...presetProfile("xbox"), name: "Empty", mappings: {} };
    const result = resolveProfileForPad(XBOX_ID, "xbox", freshStore().get(), { accountProfile: empty });
    expect(result.profile.mappings.button0).toBe("confirm");
    expect(result.profile.mappings.button1).toBe("back");
    expect(result.profile.mappings["axis0-"]).toBe("left");
    expect(result.profile.mappings["axis1+"]).toBe("down");
  });

  it("applies the glyph theme to the family, automatic following the pad", () => {
    const store = freshStore();
    const identity = padFingerprint(DUALSENSE_ID);
    store.assign(identity, { glyphTheme: "switch" });
    expect(
      resolveProfileForPad(DUALSENSE_ID, "dualsense", store.get(), {}).glyphFamily,
    ).toBe("switch");
    store.assign(identity, { glyphTheme: "auto" });
    expect(
      resolveProfileForPad(DUALSENSE_ID, "dualsense", store.get(), {}).glyphFamily,
    ).toBe("dualsense");
  });

  it("reset to automatic returns the pad to the automatic chain", () => {
    const store = freshStore();
    const identity = padFingerprint(XBOX_ID);
    store.assign(identity, { source: { kind: "preset", id: "generic" }, swapAB: true });
    store.remove(identity.fingerprint);
    const result = resolveProfileForPad(XBOX_ID, "xbox", store.get(), {
      accountProfile,
      gameProfile,
      gameId: 7,
      presets: PRESETS,
    });
    expect(result.source).toEqual({ kind: "auto" });
    expect(result.swapAB).toBe(false);
    expect(result.profile.name).toBe("Hades deck");
  });
});
