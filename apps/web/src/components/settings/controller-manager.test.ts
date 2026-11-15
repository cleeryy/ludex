import { describe, expect, it } from "vitest";
import { profileOptions } from "@/components/big-picture/controller";
import { presetProfile } from "@/lib/controller-profiles";
import {
  createControllerDevicesStore,
  sourceFromId,
  sourceId,
  type ControllerStorage,
} from "@/lib/controller-devices";
import { padFingerprint } from "@/lib/controller-identity";
import { resolveProfileForPad } from "@/lib/controller-resolve";
import type { ControllerProfilesResponse } from "@/lib/types";

/* --------------------------------------------------------------------------
   #230: the manager's selects speak the same id vocabulary as the Console
   profile overlay, and an assignment made in Settings is what the console
   resolves on the next press. The row itself is presentation; these tests pin
   the contract behind it (assign, change, reset to automatic).
   -------------------------------------------------------------------------- */

const XBOX_ID = "Xbox Wireless Controller (STANDARD GAMEPAD Vendor: 045e Product: 0b13)";

function storage(): ControllerStorage {
  const map = new Map<string, string>();
  return { getItem: (key) => map.get(key) ?? null, setItem: (key, value) => void map.set(key, value) };
}

function profilesResponse(): ControllerProfilesResponse {
  return {
    global: {
      gameId: null,
      gameTitle: null,
      profile: presetProfile("dualsense"),
      updatedAt: "2026-11-01T00:00:00Z",
    },
    games: [
      {
        gameId: 7,
        gameTitle: "Hades",
        profile: presetProfile("steamdeck"),
        updatedAt: "2026-11-02T00:00:00Z",
      },
    ],
    presets: [
      { id: "xbox", label: "Xbox", profile: presetProfile("xbox") },
      { id: "dualsense", label: "DualSense", profile: presetProfile("dualsense") },
    ],
  };
}

describe("controller manager assignment", () => {
  const data = profilesResponse();
  const options = profileOptions(data, presetProfile("xbox"));

  it("offers only ids the device map can round-trip", () => {
    expect(options.map((option) => option.id)).toEqual([
      "auto",
      "account",
      "game:7",
      "preset:xbox",
      "preset:dualsense",
    ]);
    for (const option of options) {
      expect(sourceId(sourceFromId(option.id))).toBe(option.id);
    }
  });

  it("applies an assignment to the pad that is connected now", () => {
    const store = createControllerDevicesStore({ storage: storage(), key: "test.devices", listen: false });
    const identity = padFingerprint(XBOX_ID);
    const before = resolveProfileForPad(XBOX_ID, "xbox", store.get(), {
      accountProfile: data.global?.profile,
      presets: data.presets,
    });
    expect(before.profile.name).toBe("DualSense");

    // The manager writes the option id straight into the entry.
    store.assign(identity, { source: sourceFromId("preset:dualsense") });
    const after = resolveProfileForPad(XBOX_ID, "xbox", store.get(), {
      accountProfile: data.global?.profile,
      presets: data.presets,
    });
    expect(after.source).toEqual({ kind: "preset", id: "dualsense" });
    expect(after.profile.name).toBe("DualSense");

    // Account and Automatic are one click apart.
    store.assign(identity, { source: sourceFromId("account") });
    expect(
      resolveProfileForPad(XBOX_ID, "xbox", store.get(), {
        accountProfile: data.global?.profile,
        presets: data.presets,
      }).source,
    ).toEqual({ kind: "account" });

    store.assign(identity, { source: sourceFromId("auto") });
    expect(
      resolveProfileForPad(XBOX_ID, "xbox", store.get(), {
        accountProfile: data.global?.profile,
        presets: data.presets,
      }).source,
    ).toEqual({ kind: "auto" });
  });

  it("resets the pad to automatic without touching the account profile", () => {
    const store = createControllerDevicesStore({ storage: storage(), key: "test.devices", listen: false });
    const identity = padFingerprint(XBOX_ID);
    store.assign(identity, { source: sourceFromId("preset:xbox"), glyphTheme: "numbers", swapAB: true });
    store.remove(identity.fingerprint);

    const resolved = resolveProfileForPad(XBOX_ID, "xbox", store.get(), {
      accountProfile: data.global?.profile,
      presets: data.presets,
    });
    expect(resolved.entry).toBeNull();
    expect(resolved.source).toEqual({ kind: "auto" });
    expect(resolved.glyphFamily).toBe("xbox");
    expect(resolved.swapAB).toBe(false);
    expect(resolved.profile.name).toBe("DualSense");
  });
});
