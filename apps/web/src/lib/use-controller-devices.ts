import { useEffect, useState, useSyncExternalStore } from "react";
import { padFingerprint, type PadIdentity } from "@/lib/controller-identity";
import {
  EMPTY_CONTROLLER_DEVICES,
  controllerDevices,
  type ControllerDevices,
  type ControllerDevicesStore,
} from "@/lib/controller-devices";

/* --------------------------------------------------------------------------
   React bindings for the device map (#230).

   `useConnectedPads` is the browser-facing half: a cheap poll plus the hot-plug
   events, because a pad can appear and disappear without any other signal and
   the manager must list what is plugged in right now. Each pad carries its
   fingerprint, so a row can be matched to a stored profile without ever using
   `gamepad.index` as identity.

   The hook also keeps a short ring buffer of axis samples per fingerprint; the
   generic fallback uses it to tell a d-pad hat from an analog axis without
   misreading a trigger that rests at -1.
   -------------------------------------------------------------------------- */

export interface ConnectedPad {
  index: number;
  id: string;
  identity: PadIdentity;
  /** `standard` or whatever the browser reports for a non-standard pad. */
  mapping: string;
  buttons: number;
  axes: number;
  /** Percentage when the platform exposes it; Chrome does not. */
  battery: number | null;
  charging: boolean;
}

const SAMPLE_LIMIT = 12;
const samples = new Map<string, number[][]>();

function rememberSamples(fingerprint: string, axes: readonly number[]): void {
  const values = Array.from(axes);
  const previous = samples.get(fingerprint);
  if (!previous) {
    samples.set(fingerprint, [values]);
    return;
  }
  const last = previous[previous.length - 1];
  if (last && last.length === values.length && last.every((value, index) => value === values[index])) return;
  previous.push(values);
  if (previous.length > SAMPLE_LIMIT) previous.splice(0, previous.length - SAMPLE_LIMIT);
}

/** Axis snapshots collected while the pad stayed connected. */
export function axisSamplesFor(fingerprint: string): number[][] {
  return samples.get(fingerprint) ?? [];
}

function batteryOf(pad: Gamepad): { battery: number | null; charging: boolean } {
  // Not part of the Gamepad API yet; a few WebViews and Tauri builds expose it.
  const raw = (pad as unknown as { battery?: { level?: unknown; charging?: unknown } }).battery;
  if (!raw || typeof raw !== "object") return { battery: null, charging: false };
  const level = Number(raw.level);
  return {
    battery: Number.isFinite(level) ? Math.round(Math.max(0, Math.min(1, level)) * 100) : null,
    charging: raw.charging === true,
  };
}

/** Snapshot of the pads the browser currently exposes, ordered by index. */
export function readConnectedPads(): ConnectedPad[] {
  if (typeof navigator === "undefined" || typeof navigator.getGamepads !== "function") return [];
  let raw: Array<Gamepad | null>;
  try {
    raw = Array.from(navigator.getGamepads() ?? []);
  } catch {
    // Some WebViews throw in an insecure context; that is "no pad".
    return [];
  }
  return raw
    .filter((pad): pad is Gamepad => Boolean(pad && pad.connected))
    .sort((left, right) => left.index - right.index)
    .map((pad) => {
      const identity = padFingerprint(pad.id, undefined, { mapping: pad.mapping });
      rememberSamples(identity.fingerprint, pad.axes);
      const { battery, charging } = batteryOf(pad);
      return {
        index: pad.index,
        id: pad.id,
        identity,
        mapping: pad.mapping,
        buttons: pad.buttons.length,
        axes: pad.axes.length,
        battery,
        charging,
      };
    });
}

function samePads(left: readonly ConnectedPad[], right: readonly ConnectedPad[]): boolean {
  if (left.length !== right.length) return false;
  return left.every((pad, index) => {
    const other = right[index];
    return (
      other !== undefined &&
      pad.index === other.index &&
      pad.id === other.id &&
      pad.mapping === other.mapping &&
      pad.buttons === other.buttons &&
      pad.axes === other.axes &&
      pad.battery === other.battery &&
      pad.charging === other.charging
    );
  });
}

/** Connected pads, refreshed on hot-plug, visibility and a one-second poll. */
export function useConnectedPads(): ConnectedPad[] {
  const [pads, setPads] = useState<ConnectedPad[]>(readConnectedPads);

  useEffect(() => {
    let live = true;
    const refresh = () => {
      if (!live) return;
      const next = readConnectedPads();
      setPads((previous) => (samePads(previous, next) ? previous : next));
    };
    refresh();
    const timer = window.setInterval(refresh, 1_000);
    window.addEventListener("gamepadconnected", refresh);
    window.addEventListener("gamepaddisconnected", refresh);
    document.addEventListener("visibilitychange", refresh);
    return () => {
      live = false;
      window.clearInterval(timer);
      window.removeEventListener("gamepadconnected", refresh);
      window.removeEventListener("gamepaddisconnected", refresh);
      document.removeEventListener("visibilitychange", refresh);
    };
  }, []);

  return pads;
}

/** The pad the console should talk about: the last used one when present. */
export function pickActivePad(
  pads: readonly ConnectedPad[],
  last: string | null | undefined,
): ConnectedPad | null {
  if (pads.length === 0) return null;
  if (last) {
    const remembered = pads.find((pad) => pad.identity.fingerprint === last);
    if (remembered) return remembered;
  }
  return pads[0] ?? null;
}

/** [device map, store] — the store is stable, the map re-renders on change. */
export function useControllerDevices(): readonly [ControllerDevices, ControllerDevicesStore] {
  const devices = useSyncExternalStore(
    controllerDevices.subscribe,
    controllerDevices.get,
    () => EMPTY_CONTROLLER_DEVICES,
  );
  return [devices, controllerDevices] as const;
}
