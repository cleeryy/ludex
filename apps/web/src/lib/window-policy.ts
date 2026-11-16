import { useEffect, useSyncExternalStore } from "react";
import { isNative, native, type WindowPolicy } from "@/lib/native";

/**
 * The native window policy (#269).
 *
 * Rust resolves decorations, fullscreen, tray, close behaviour and the overlay
 * host once from `{argv, console session, settings}` and applies it. The web
 * reads it instead of re-deriving fullscreen from `console_mode()` plus
 * `isFullscreen()`, the split that produced the #233 fullscreen trap.
 *
 * Three states matter to callers:
 * - `undefined`: the policy is not loaded yet (do not touch the window);
 * - `null`: the desktop does not expose one (older build, browser) — keep the
 *   pre-#269 behaviour;
 * - a policy: treat it as the only source of truth.
 */

type Listener = () => void;

const listeners = new Set<Listener>();
let snapshot: WindowPolicy | null | undefined;
let started = false;

function notify(): void {
  for (const listener of listeners) listener();
}

function setSnapshot(next: WindowPolicy | null): void {
  if (next === snapshot) return;
  snapshot = next;
  notify();
}

export function windowPolicySnapshot(): WindowPolicy | null | undefined {
  return snapshot;
}

function subscribe(listener: Listener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** Load the policy once, then keep it live through `window://state`. */
export function hydrateWindowPolicy(): void {
  if (started || !isNative()) return;
  started = true;
  void (async () => {
    try {
      const initial = await native.windowState();
      setSnapshot(initial);
      await native.onWindowState((policy) => setSnapshot(policy));
    } catch {
      // A desktop without the command keeps the legacy inference; allow a later
      // mount to retry once the IPC is ready.
      setSnapshot(null);
      started = false;
    }
  })();
}

/**
 * Subscribe to the policy. `undefined` while loading, `null` when the desktop
 * has none, the resolved policy otherwise.
 */
export function useWindowPolicy(): WindowPolicy | null | undefined {
  const value = useSyncExternalStore(subscribe, windowPolicySnapshot, () => null);
  useEffect(() => {
    hydrateWindowPolicy();
  }, []);
  return value;
}
