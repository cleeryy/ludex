import type { Page } from "@playwright/test";

/**
 * Minimal Tauri bridge for the console specs (#302).
 *
 * The browser build is a companion: it browses and downloads, but it cannot
 * install, launch or sleep. Three of the #302 acceptance flows only exist on
 * the native path (play -> launch plate, install -> Downloads job, Power ->
 * confirm), so those specs install this bridge before the bundle loads and let
 * the app take its real `isNative()` branch. Nothing here mocks a React hook or
 * the input loop: the fake pad still drives `useGamepad`, the shell still
 * dispatches one command per press, and the assertions read the same DOM a
 * desktop build renders.
 *
 * The bridge answers the handful of commands the Console issues at boot and
 * returns `null` for everything else, so an unimplemented command degrades to
 * the "older desktop" path instead of crashing the renderer. `invoke` records
 * command names only — never arguments, which carry the session token.
 *
 * ```ts
 * const bridge = await installNativeBridge(page, {
 *   serverUrl: process.env.LUDEX_E2E_BASE_URL!,
 *   token: process.env.LUDEX_E2E_TOKEN!,
 *   installed: [{ gameId }],   // the seeded game looks installed: Play, not Install
 *   holdLaunch: true,          // launch_game stays pending so the plate reaches "slow"
 * });
 * await page.goto(consoleUrl());
 * await bridge.commands();     // ["get_settings", "console_mode", ...]
 * ```
 */

export interface NativeInstalledGame {
  gameId: number;
  versionId?: number;
  title?: string;
  version?: string | null;
  executable?: string | null;
}

export interface NativeBridgeOptions {
  /** Server the desktop settings point at; the bridge does not guess it. */
  serverUrl: string;
  /** Session token the native settings carry (never logged). */
  token: string;
  /** Games the desktop reports as installed. */
  installed?: NativeInstalledGame[];
  /**
   * `launch_game` never settles. The launch plate then reaches its 8 s "slow"
   * state, which is the only state `B` may dismiss; a resolved launch closes
   * the plate by itself and would not exercise the back path.
   */
  holdLaunch?: boolean;
}

export interface NativeBridge {
  /** Command names invoked so far, in order (arguments are never recorded). */
  commands(): Promise<string[]>;
  /** How many times one command ran; a cancelled power confirm must see 0. */
  callCount(command: string): Promise<number>;
  /** Push one Tauri event to the app's registered listeners. */
  emit(event: string, payload: unknown): Promise<void>;
  /** Replace the installed list (the next `list_installed` answers it). */
  setInstalled(games: NativeInstalledGame[]): Promise<void>;
  /** Every job this bridge created through `install_game`. */
  jobs(): Promise<Array<Record<string, unknown>>>;
}

type NativeWindow = Window & {
  __ludexNative: {
    calls: () => string[];
    emit: (event: string, payload: unknown) => void;
    setInstalled: (games: NativeInstalledGame[]) => void;
    jobs: () => Array<Record<string, unknown>>;
  };
};

/**
 * Serialized into the page by `addInitScript`; it may only use its argument and
 * page globals. Kept dependency-free on purpose.
 */
function installNativeBridgeScript(options: NativeBridgeOptions): void {
  const calls: string[] = [];
  const callbacks = new Map<number, (event: unknown) => void>();
  const listeners = new Map<string, number[]>();
  let nextCallback = 1;
  let installed = (options.installed ?? []).slice();
  let jobs: Array<Record<string, unknown>> = [];
  let jobSeq = 0;

  const settings = {
    mode: "local",
    diagnosticsEnabled: false,
    diagnosticsInstallId: null,
    deviceId: "ludex-e2e-device",
    deviceName: "Ludex E2E",
    serverUrl: options.serverUrl,
    token: options.token,
    gamesDir: null,
    installDir: "/tmp/ludex-e2e/install",
    installRootConfirmed: true,
    downloadDir: "/tmp/ludex-e2e/downloads",
    locations: [],
    serverPort: 0,
    streamInstall: true,
    downloadPolicy: "automatic",
  };

  const installedGames = () =>
    installed.map((game) => ({
      gameId: game.gameId,
      versionId: game.versionId ?? 1,
      title: game.title ?? "Ludex E2E Game",
      version: game.version ?? "1.0.0",
      installPath: `/tmp/ludex-e2e/install/${game.gameId}`,
      executable: game.executable ?? "game.exe",
      sizeBytes: 1024,
      installedAt: "2026-11-19T00:00:00Z",
      lastLaunchedAt: null,
      source: "ludex",
      launchUri: null,
    }));

  const makeJob = (args: Record<string, unknown>): Record<string, unknown> => {
    jobSeq += 1;
    const gameId = Number(args.gameId ?? 0);
    return {
      jobId: `e2e-job-${jobSeq}`,
      gameId,
      versionId: Number(args.versionId ?? 0),
      phase: "downloading",
      bytesDone: 1_200_000,
      bytesTotal: 2_400_000,
      speedBps: 0,
      message: null,
      installPath:
        typeof args.installDir === "string" && args.installDir
          ? args.installDir
          : `/tmp/ludex-e2e/install/${gameId}`,
      stream: false,
      resumable: false,
      entriesDone: null,
      entriesTotal: null,
      currentEntry: null,
      resumedEntries: null,
      reconnecting: false,
      attempt: 1,
      lastError: null,
      stalledForMs: null,
      nextRetryAt: null,
      httpStatus: null,
      waitingForNetwork: false,
      queuePosition: 1,
      prefetching: false,
      space: null,
      verifiedSha256: null,
    };
  };

  // Async on purpose: `@tauri-apps/api/event` chains `.then()` on the returned
  // value in `listen()`, so a synchronous answer would break every listener.
  const invoke = async (command: string, args: Record<string, unknown> = {}): Promise<unknown> => {
    // Names only: install_game carries the session token in its arguments.
    calls.push(command);
    switch (command) {
      case "get_settings":
      case "local_settings":
        return { ...settings };
      case "save_settings":
        return { ...settings };
      case "console_mode":
        // A desktop console, not a Sunshine session: the Power list keeps the
        // "Exit Console mode" row and the shell does not claim to own fullscreen.
        return { active: false, fullscreen: false, appName: "Ludex", exitShortcut: "f12" };
      case "window_state":
        // `tray: false` means Rust owns the window: the shell never calls the
        // Tauri window API (which this bridge does not implement).
        return {
          decorations: false,
          fullscreen: true,
          tray: false,
          closeAction: "close",
          overlayHost: "disabled",
        };
      case "overlay_status":
        return null;
      case "sleep_capability":
        return { available: true, backend: "e2e", reason: null };
      case "list_installed":
        return installedGames();
      case "list_install_jobs":
        return jobs.slice();
      case "install_game": {
        const job = makeJob(args);
        jobs = [job, ...jobs];
        return job;
      }
      case "launch_game":
        // The native call has no abort. Holding it lets the plate speak its
        // honest 8 s line and makes `B` the only way out, which is the rule
        // this spec verifies. A resolved launch would close the plate itself.
        return options.holdLaunch === false
          ? Promise.resolve({ sessionId: "e2e-session", gameId: Number(args.gameId ?? 0) })
          : new Promise(() => undefined);
      case "plugin:event|listen": {
        const event = String(args.event ?? "");
        const handler = Number(args.handler ?? 0);
        const ids = listeners.get(event) ?? [];
        ids.push(handler);
        listeners.set(event, ids);
        return ids.length;
      }
      case "plugin:event|unlisten":
        return null;
      default:
        return null;
    }
  };

  const bridge = {
    calls: () => calls.slice(),
    emit(event: string, payload: unknown) {
      for (const id of listeners.get(event) ?? []) {
        callbacks.get(id)?.({ event, id, payload });
      }
    },
    setInstalled(games: NativeInstalledGame[]) {
      installed = games.slice();
    },
    jobs: () => jobs.slice(),
  };

  const internals = {
    invoke,
    transformCallback(callback: (event: unknown) => void, once = false) {
      const id = nextCallback;
      nextCallback += 1;
      callbacks.set(id, (payload) => {
        if (once) callbacks.delete(id);
        callback(payload);
      });
      return id;
    },
    metadata: {
      currentWindow: { label: "main" },
      currentWebview: { windowLabel: "main", label: "main" },
    },
    convertFileSrc: (path: string) => path,
  };

  Object.defineProperty(window, "__TAURI_INTERNALS__", {
    configurable: true,
    value: internals,
  });
  // `@tauri-apps/api/event` unlisten removes the listener through this global
  // before it invokes `plugin:event|unlisten`. Without it, every scope cleanup
  // that unsubscribes throws `undefined.unregisterListener` (seen on the Power
  // surface's overlay scope). The real injected script provides it; so do we.
  Object.defineProperty(window, "__TAURI_EVENT_PLUGIN_INTERNALS__", {
    configurable: true,
    value: {
      unregisterListener: () => undefined,
      emit: () => undefined,
    },
  });
  (window as unknown as NativeWindow).__ludexNative = bridge;
}

export async function installNativeBridge(
  page: Page,
  options: NativeBridgeOptions,
): Promise<NativeBridge> {
  await page.addInitScript(installNativeBridgeScript, options);

  return {
    async commands() {
      return page.evaluate(() => (window as unknown as NativeWindow).__ludexNative.calls());
    },
    async callCount(command: string) {
      const commands = await page.evaluate(() =>
        (window as unknown as NativeWindow).__ludexNative.calls(),
      );
      return commands.filter((name) => name === command).length;
    },
    async emit(event: string, payload: unknown) {
      await page.evaluate(
        ({ eventName, data }) =>
          (window as unknown as NativeWindow).__ludexNative.emit(eventName, data),
        { eventName: event, data: payload },
      );
    },
    async setInstalled(games: NativeInstalledGame[]) {
      await page.evaluate(
        (next) => (window as unknown as NativeWindow).__ludexNative.setInstalled(next),
        games,
      );
    },
    async jobs() {
      return page.evaluate(() => (window as unknown as NativeWindow).__ludexNative.jobs());
    },
  };
}
