import { describe, expect, it } from "vitest";
import { padFingerprint } from "./controller-identity";
import {
  AUTO_SOURCE_ID,
  CONTROLLER_DEVICES_KEY,
  createControllerDevicesStore,
  readControllerDevices,
  sanitizeControllerDevices,
  sourceFromId,
  sourceId,
  type ControllerStorage,
} from "./controller-devices";

/* --------------------------------------------------------------------------
   #230: the device map is the only place a pad's profile choice lives. It is
   device-local and unscoped, so these tests only need a fake `localStorage`.
   -------------------------------------------------------------------------- */

const XBOX = padFingerprint("Xbox Wireless Controller (STANDARD GAMEPAD Vendor: 045e Product: 0b13)");
const DUALSENSE = padFingerprint("DualSense Wireless Controller (STANDARD GAMEPAD Vendor: 054c Product: 0ce6)");

function memoryStorage(entries: Record<string, string> = {}) {
  const map = new Map(Object.entries(entries));
  const storage: ControllerStorage & { map: Map<string, string> } = {
    map,
    getItem: (key) => map.get(key) ?? null,
    setItem: (key, value) => {
      map.set(key, value);
    },
  };
  return storage;
}

describe("controller device store", () => {
  it("round-trips an assignment through storage", () => {
    const storage = memoryStorage();
    const store = createControllerDevicesStore({ storage, key: "test.devices", listen: false, now: () => 42 });
    store.assign(XBOX, { source: { kind: "preset", id: "dualsense" }, glyphTheme: "playstation", swapAB: true });

    expect(store.get().pads[XBOX.fingerprint]).toMatchObject({
      label: "Xbox",
      source: { kind: "preset", id: "dualsense" },
      glyphTheme: "playstation",
      swapAB: true,
      updatedAt: 42,
    });

    const raw = JSON.parse(storage.map.get("test.devices") ?? "null");
    expect(raw).toMatchObject({ version: 1, pads: { [XBOX.fingerprint]: { swapAB: true } } });

    // A fresh store (next launch) reads the same entry back.
    const reloaded = createControllerDevicesStore({ storage, key: "test.devices", listen: false });
    expect(reloaded.get().pads[XBOX.fingerprint]?.glyphTheme).toBe("playstation");
  });

  it("keeps an existing label and source when a later patch only changes the theme", () => {
    const store = createControllerDevicesStore({ storage: memoryStorage(), key: "test.devices", listen: false });
    store.assign(XBOX, { source: { kind: "account" }, label: "Couch pad" });
    store.assign(XBOX, { glyphTheme: "numbers" });
    expect(store.get().pads[XBOX.fingerprint]).toMatchObject({
      label: "Couch pad",
      source: { kind: "account" },
      glyphTheme: "numbers",
      swapAB: false,
    });
  });

  it("stores one entry per fingerprint, so two identical pads share a profile", () => {
    const store = createControllerDevicesStore({ storage: memoryStorage(), key: "test.devices", listen: false });
    store.assign(XBOX, { source: { kind: "preset", id: "xbox" } });
    store.assign(XBOX, { swapAB: true });
    expect(Object.keys(store.get().pads)).toEqual([XBOX.fingerprint]);
  });

  it("tracks the last pad that drove the console and forgets it on removal", () => {
    const store = createControllerDevicesStore({ storage: memoryStorage(), key: "test.devices", listen: false });
    store.assign(XBOX, {});
    store.assign(DUALSENSE, {});
    store.touch(XBOX.fingerprint);
    expect(store.get().last).toBe(XBOX.fingerprint);
    // Touching the same pad twice writes nothing.
    const before = store.get();
    store.touch(XBOX.fingerprint);
    expect(store.get()).toBe(before);
    store.remove(XBOX.fingerprint);
    expect(store.get().last).toBeNull();
    expect(store.get().pads[DUALSENSE.fingerprint]).toBeDefined();
  });

  it("resets to automatic by removing the entry only", () => {
    const store = createControllerDevicesStore({ storage: memoryStorage(), key: "test.devices", listen: false });
    store.assign(DUALSENSE, { source: { kind: "preset", id: "dualsense" }, swapAB: true });
    store.remove(DUALSENSE.fingerprint);
    expect(store.get().pads[DUALSENSE.fingerprint]).toBeUndefined();
    // The account/game profiles live on the server and are untouched.
    expect(store.get().version).toBe(1);
  });

  it("sanitises hand-edited storage: wrong key, bad source, bad theme, bad label", () => {
    const payload = {
      version: 1,
      last: "usb:045e:0b13",
      pads: {
        "usb:045e:0b13": {
          label: "  Couch  ",
          identity: { rawId: XBOX.rawId, family: "xbox", standard: true },
          source: { kind: "game", gameId: -4 },
          glyphTheme: "loud",
          swapAB: "yes",
          updatedAt: "yesterday",
        },
        "usb:0000:1111": {
          // Key does not match the id: a tampered or stale entry is dropped.
          label: "Ghost",
          identity: { rawId: DUALSENSE.rawId },
        },
        "usb:054c:0ce6": {
          label: "",
          identity: { rawId: DUALSENSE.rawId, family: "dualsense", standard: true },
          source: { kind: "preset", id: "dualsense" },
          glyphTheme: "switch",
          swapAB: true,
          updatedAt: 7,
        },
      },
    };
    const devices = sanitizeControllerDevices(payload);
    expect(Object.keys(devices.pads).sort()).toEqual(["usb:045e:0b13", "usb:054c:0ce6"]);
    expect(devices.pads["usb:045e:0b13"]).toMatchObject({
      label: "Couch",
      source: { kind: "auto" },
      glyphTheme: "auto",
      swapAB: false,
      updatedAt: 0,
    });
    // An empty label is filled from the recomputed identity.
    expect(devices.pads["usb:054c:0ce6"]?.label).toBe("DualSense");
  });

  it("starts empty on garbage, and drops a last that is not a string", () => {
    expect(sanitizeControllerDevices(null)).toEqual({ version: 1, pads: {}, last: null });
    expect(readControllerDevices(memoryStorage({ "test.devices": "{oops" }), "test.devices")).toEqual({
      version: 1,
      pads: {},
      last: null,
    });
    expect(
      readControllerDevices(memoryStorage({ "test.devices": JSON.stringify({ pads: {}, last: 12 }) }), "test.devices").last,
    ).toBeNull();
  });

  it("uses the unscoped key by default", () => {
    const storage = memoryStorage();
    const store = createControllerDevicesStore({ storage, listen: false });
    store.assign(XBOX, { swapAB: true });
    expect(storage.map.has(CONTROLLER_DEVICES_KEY)).toBe(true);
    expect(CONTROLLER_DEVICES_KEY).toBe("ludex.controllers.v1");
  });
});

describe("device source ids", () => {
  it("round-trips the Console vocabulary", () => {
    expect(sourceId({ kind: "auto" })).toBe(AUTO_SOURCE_ID);
    expect(sourceId({ kind: "account" })).toBe("account");
    expect(sourceId({ kind: "game", gameId: 7 })).toBe("game:7");
    expect(sourceId({ kind: "preset", id: "dualsense" })).toBe("preset:dualsense");
    expect(sourceFromId("game:7")).toEqual({ kind: "game", gameId: 7 });
    expect(sourceFromId("preset:dualsense")).toEqual({ kind: "preset", id: "dualsense" });
    expect(sourceFromId("auto")).toEqual({ kind: "auto" });
    expect(sourceFromId(null)).toEqual({ kind: "auto" });
  });

  it("falls back to automatic on an id it cannot parse", () => {
    expect(sourceFromId("game:0")).toEqual({ kind: "auto" });
    expect(sourceFromId("game:abc")).toEqual({ kind: "auto" });
    expect(sourceFromId("preset:")).toEqual({ kind: "auto" });
    expect(sourceFromId("nonsense")).toEqual({ kind: "auto" });
  });
});
