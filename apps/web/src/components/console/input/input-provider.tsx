import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import {
  gamepadDebugEnabled,
  useGamepad,
  type ActionSource,
  type GamepadAction,
  type GamepadHandle,
} from "@/lib/gamepad";
import type { ControllerProfile } from "@/lib/types";
import { useControllerProfile } from "@/lib/use-controller-profile";
import type { CueId } from "@/lib/ui-sound-specs";
import type { HapticsStrength, PadLike } from "@/lib/gamepad-core";
import { captureFocus, restoreFocus, type FocusSnapshot } from "@/components/console/input/focus-store";

/**
 * One input owner for the whole app (#267).
 *
 * Before this, four components ran their own `useGamepad` polling loop: the
 * Console, the library grid, the in-app overlay and the onboarding tour. With
 * the overlay open, the Console underneath received the same press, so one
 * physical A ran one action twice.
 *
 * `InputProvider` owns the single polling loop and a **scope stack**. A
 * surface registers a scope while it is mounted; only the top enabled scope
 * receives pad actions, and every scope — Console, Library, overlay, tour —
 * consumes the same command vocabulary from `input.ts` / `controller.ts`.
 * Pushing a scope snapshots the focus owner; popping it restores that exact
 * element, so B on a menu lands on the card the menu covered.
 *
 * Keyboard and remote keys go through the same top-scope dispatcher: a scope
 * that supplies `onKey` gets first refusal on `keydown`, which is how the
 * Console's key map and the overlay's close key share one owner.
 */

export interface InputScopeOptions {
  profile?: ControllerProfile | null;
  resolveProfile?: (pad: PadLike | null) => ControllerProfile | null;
  haptics?: boolean;
  hapticsStrength?: HapticsStrength;
  /** The surface speaks in cues and pulses its own patterns. */
  cueHaptics?: boolean;
}

export interface InputScope {
  id: string;
  /** Pad actions. Only the top scope's handler runs. */
  onAction?: (action: GamepadAction, source: ActionSource | null, repeated?: boolean) => void;
  /** Keyboard / remote. Return `true` when the event was consumed. */
  onKey?: (event: KeyboardEvent) => boolean | void;
  enabled?: boolean;
  /** This scope draws a `data-pad-layer`; its own layer must not block it. */
  padLayer?: boolean;
  options?: InputScopeOptions;
}

interface RegisteredScope {
  id: string;
  scope: { current: InputScope };
  snapshot: FocusSnapshot;
}

export type LastInputSource = "pad" | "keyboard" | "pointer";

export interface InputApi extends GamepadHandle {
  /** Dispatch a logical action to the top scope (tests and remote bridges). */
  dispatch: (action: GamepadAction, source?: ActionSource | null, repeated?: boolean) => void;
  /** Push a scope; the returned function pops it and restores focus. */
  pushScope: (id: string, scope: { current: InputScope }) => () => void;
  /** Re-read the top scope's options (a console preference changed). */
  refresh: () => void;
  /** The top enabled scope's id, or `null` when no surface wants input. */
  activeScope: string | null;
  /** Most recent input modality so hints can match controller, keyboard or pointer. */
  lastInput: LastInputSource;
}

const InputContext = createContext<InputApi | null>(null);

/** TV remote / media keys normalized to logical gamepad actions. */
function remoteActionForKey(key: string): GamepadAction | null {
  if (key === "GoBack" || key === "BrowserBack") return "back";
  if (key === "MediaPlayPause") return "confirm";
  return null;
}

export function InputProvider({ children }: { children: ReactNode }) {
  const controllerProfile = useControllerProfile();
  const scopesRef = useRef<RegisteredScope[]>([]);
  const [topVersion, setTopVersion] = useState(0);
  const [lastInput, setLastInput] = useState<LastInputSource>("pad");
  const debug = gamepadDebugEnabled();

  const topScope = useCallback((): RegisteredScope | null => {
    for (let index = scopesRef.current.length - 1; index >= 0; index -= 1) {
      const entry = scopesRef.current[index];
      if (entry.scope.current.enabled !== false) return entry;
    }
    return null;
  }, []);

  const dispatch = useCallback(
    (action: GamepadAction, source: ActionSource | null = null, repeated = false) => {
      setLastInput("pad");
      const top = topScope();
      top?.scope.current.onAction?.(action, source, repeated);
    },
    [topScope],
  );

  const pushScope = useCallback(
    (id: string, scope: { current: InputScope }) => {
      const entry: RegisteredScope = { id, scope, snapshot: captureFocus() };
      scopesRef.current = [...scopesRef.current, entry];
      setTopVersion((version) => version + 1);
      return () => {
        const index = scopesRef.current.findIndex((candidate) => candidate.id === id && candidate.scope === scope);
        if (index < 0) return;
        const wasTop = index === scopesRef.current.length - 1;
        scopesRef.current = scopesRef.current.filter((_, at) => at !== index);
        setTopVersion((version) => version + 1);
        // Only the surface that owned the ring hands it back. A scope that was
        // buried under another one never had focus to restore.
        if (wasTop) restoreFocus(entry.snapshot);
      };
    },
    [],
  );

  // The top scope supplies the pad options: the Console resolves per-device
  // profiles and speaks in cues, the library uses the account profile.
  const top = useMemo(() => {
    void topVersion;
    return topScope();
  }, [topScope, topVersion]);

  const profileRef = useRef(controllerProfile);
  profileRef.current = controllerProfile;
  const options = top?.scope.current.options ?? {};
  const resolveProfile = useCallback(
    (pad: PadLike | null) => topScope()?.scope.current.options?.resolveProfile?.(pad) ?? profileRef.current,
    // Re-resolve when the owning scope or its options change (#267): the
    // Console's per-device profile must follow its preference instantly.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [topScope, topVersion, top?.id],
  );

  const refresh = useCallback(() => setTopVersion((version) => version + 1), []);

  const gamepad = useGamepad(
    (action, source, repeated) => dispatch(action, source, repeated),
    {
      profile: options.profile ?? controllerProfile,
      resolveProfile,
      debug,
      haptics: options.haptics ?? true,
      hapticsStrength: options.hapticsStrength,
      cueHaptics: options.cueHaptics ?? false,
      owner: top?.scope.current.padLayer ?? false,
    },
  );

  // One keyboard/remote listener. Only a scope that asked for keys sees them,
  // so a desktop screen's own shortcuts keep working untouched. TV remote keys
  // (`GoBack`, `BrowserBack`, `MediaPlayPause`) fall back to the logical action
  // when the scope did not consume the raw key.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      setLastInput("keyboard");
      const current = topScope();
      const handler = current?.scope.current.onKey;
      const consumed = handler ? handler(event) === true : false;
      if (consumed || event.defaultPrevented) return;
      const remote = remoteActionForKey(event.key);
      if (remote && current?.scope.current.onAction) {
        event.preventDefault();
        current.scope.current.onAction(remote, null, event.repeat);
      }
    };
    const onPointerDown = () => setLastInput("pointer");
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("pointerdown", onPointerDown, { passive: true });
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("pointerdown", onPointerDown);
    };
  }, [topScope]);

  const api = useMemo<InputApi>(
    () => ({
      ...gamepad,
      dispatch,
      pushScope,
      refresh,
      activeScope: top?.id ?? null,
      lastInput,
    }),
    [gamepad, dispatch, pushScope, refresh, top?.id, lastInput],
  );

  return <InputContext.Provider value={api}>{children}</InputContext.Provider>;
}

export function useInput(): InputApi {
  const value = useContext(InputContext);
  if (!value) throw new Error("useInput must be used inside InputProvider");
  return value;
}

/**
 * Register a surface while it is mounted. Handlers are read through a ref, so
 * a scope with changing closures does not re-register (and does not shuffle
 * the stack) on every render.
 */
export function useInputScope(scope: InputScope): { activeScope: string | null } {
  const api = useInput();
  const scopeRef = useRef(scope);
  scopeRef.current = scope;
  const idRef = useRef(scope.id);
  const pushScope = api.pushScope;
  useEffect(() => {
    return pushScope(idRef.current, scopeRef);
  }, [pushScope]);
  // The provider reads the top scope's options through a ref; a device
  // preference change (haptics, cue mode, profile choice) must re-read them.
  const refresh = api.refresh;
  const profile = scope.options?.profile;
  const resolver = scope.options?.resolveProfile;
  const haptics = scope.options?.haptics;
  const strength = scope.options?.hapticsStrength;
  const cueHaptics = scope.options?.cueHaptics;
  useEffect(() => {
    refresh();
  }, [refresh, profile, resolver, haptics, strength, cueHaptics]);
  return { activeScope: api.activeScope };
}

/** Send a cue's haptic pattern through the one pad the loop owns. */
export function useInputPulse(): (cue: CueId) => void {
  return useInput().pulse;
}
