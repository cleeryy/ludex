// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { GameDetail, LaunchOptions } from "@/lib/types";
import { changedFields } from "@/lib/settings-tiers";

/**
 * #299 render tests: a first run never sees tier 3, the unlock survives a
 * reload, and a tier-2 switch saves exactly one profile field.
 *
 * The desktop bridge is faked at the module boundary; the tiers, the mapping
 * and the copy are the production modules.
 */

const mocks = vi.hoisted(() => ({
  getLaunchOptions: vi.fn(),
  setLaunchOptions: vi.fn(),
  compatScan: vi.fn(),
  winePrefixInfo: vi.fn(),
  makeItRunDiagnose: vi.fn(),
  onRuntimeProgress: vi.fn(),
  overlayStatus: vi.fn(),
  setOverlayGameDisabled: vi.fn(),
  launchGame: vi.fn(),
  makeItRunApply: vi.fn(),
}));

vi.mock("@/lib/native", () => ({
  isNative: () => true,
  native: mocks,
}));

vi.mock("@/state/toasts", () => ({
  useToast: () => ({ push: () => undefined }),
}));

import { SettingsTiersPanel } from "@/components/game/settings-tiers";
import { ADVANCED_TIERS_STORAGE_KEY } from "@/lib/settings-tiers";

const BASE_OPTIONS: LaunchOptions = {
  mode: "auto",
  toolId: "",
  prefixPath: "",
  env: {},
  winetricksVerbs: [],
  steamAppId: null,
  workingDir: "",
  display: {
    mode: "desktop",
    width: 0,
    height: 0,
    refresh: 0,
    filter: "",
    adaptiveSync: false,
    hdr: false,
  },
};

const SCAN = {
  platform: "linux",
  linux: true,
  steamRoots: [],
  tools: [],
  umu: null,
  proton: null,
  wine: null,
  winetricks: null,
  protonfixes: null,
  defaultMode: "auto",
  steamClientPath: null,
  suggestedVerbs: [],
  warnings: [],
  gamescope: { path: "/usr/bin/gamescope", version: "3.16", hdr: true, adaptiveSync: true, filters: ["fsr"] },
  displayCapabilities: { outputs: [], active: null },
};

const DIAGNOSIS = {
  gameId: 7,
  title: "Hades",
  platform: "linux",
  arch: "x86_64",
  osVersion: null,
  elevated: false,
  windowsGame: true,
  ready: true,
  running: false,
  summary: "Everything is installed.",
  issues: [],
  steps: [],
  detections: [],
  prerequisites: [],
  gpu: { vulkan: true, note: "" },
  cacheDir: "",
  freeBytes: null,
  requiredBytes: 0,
  managed: [],
  audit: [],
  notices: [],
  generatedAt: "2026-11-19T00:00:00Z",
};

const GAME = {
  id: 7,
  title: "Hades",
  kind: "game",
  launch: {
    executable: "Hades.exe",
    parameters: "",
    installerExecutable: null,
    uninstallerExecutable: null,
  },
} as unknown as GameDetail;

let current: LaunchOptions;

function renderPanel({ isEditor = false }: { isEditor?: boolean } = {}) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter>
        <SettingsTiersPanel gameId={7} title="Hades" game={GAME} isEditor={isEditor} />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

/** The switch row's explanation, so a click target never depends on layout. */
function switchFor(label: string): HTMLElement {
  return screen.getByRole("switch", { name: label });
}

beforeAll(() => {
  if (!window.matchMedia) {
    Object.defineProperty(window, "matchMedia", {
      writable: true,
      value: (query: string) => ({
        matches: false,
        media: query,
        onchange: null,
        addListener: () => {},
        removeListener: () => {},
        addEventListener: () => {},
        removeEventListener: () => {},
        dispatchEvent: () => false,
      }),
    });
  }
});

beforeEach(() => {
  window.localStorage.clear();
  current = { ...BASE_OPTIONS, display: { ...BASE_OPTIONS.display }, env: {} };
  for (const mock of Object.values(mocks)) mock.mockReset();
  mocks.getLaunchOptions.mockImplementation(async () => current);
  mocks.setLaunchOptions.mockImplementation(async (_gameId: number, next: LaunchOptions) => {
    current = next;
    return next;
  });
  mocks.compatScan.mockResolvedValue(SCAN);
  mocks.winePrefixInfo.mockResolvedValue({
    gameId: 7,
    path: "/games/Hades/.prefix",
    custom: false,
    exists: false,
    sizeBytes: 0,
  });
  mocks.makeItRunDiagnose.mockResolvedValue(DIAGNOSIS);
  mocks.onRuntimeProgress.mockResolvedValue(() => undefined);
  mocks.overlayStatus.mockResolvedValue({ disabledGames: [] });
  mocks.setOverlayGameDisabled.mockResolvedValue({ disabledGames: [7] });
  mocks.launchGame.mockResolvedValue({ sessionId: "s1", tool: null, source: "local" });
  mocks.makeItRunApply.mockResolvedValue({
    gameId: 7,
    consent: true,
    cancelled: false,
    failed: false,
    needsElevation: false,
    steps: [],
    diagnosis: DIAGNOSIS,
    wizard: "idle",
    logPath: "",
  });
});

afterEach(() => {
  cleanup();
});

describe("tier visibility", () => {
  it("never shows tier 3 on a first run", async () => {
    renderPanel();
    await screen.findByText("Ready to play");

    expect(screen.getByText("Unlock advanced view")).toBeTruthy();
    expect(screen.queryByText("Environment overrides")).toBeNull();
    expect(screen.queryByText("Wine prefix")).toBeNull();
    expect(screen.getByText(/Raw environment variables/)).toBeTruthy();
    expect(window.localStorage.getItem(ADVANCED_TIERS_STORAGE_KEY)).toBeNull();
  });

  it("unlocks the old launch panel and remembers the choice", async () => {
    const first = renderPanel();
    await screen.findByText("Ready to play");

    screen.getByText("Unlock advanced view").click();
    await screen.findByText("Environment overrides");
    expect(window.localStorage.getItem(ADVANCED_TIERS_STORAGE_KEY)).toBe("1");

    first.unmount();
    renderPanel();
    await screen.findByText("Ready to play");
    // Persisted: the raw panel is present without a second unlock click.
    expect(await screen.findByText("Environment overrides")).toBeTruthy();
    expect(screen.queryByText("Unlock advanced view")).toBeNull();
  });
});

describe("tier 2", () => {
  it("shows a why-line for every switch", async () => {
    renderPanel();
    await screen.findByText("Ready to play");

    for (const label of ["Game window", "Frame limit", "HDR", "Adaptive sync", "In-game overlay", "Audio device"]) {
      expect(switchFor(label), label).toBeTruthy();
    }
    expect(screen.getByText("Runs on your desktop, untouched.")).toBeTruthy();
    // The three display switches wait for the game window; one shared reason.
    expect(screen.getAllByText("Turn on the game window first; this applies inside it.")).toHaveLength(3);
    expect(screen.getByText("Shift+Tab and the Home button open Ludex over the game.")).toBeTruthy();
    expect(screen.getByText("Sound follows the system default device.")).toBeTruthy();
  });

  it("writes one documented field per switch", async () => {
    renderPanel();
    await screen.findByText("Ready to play");
    await waitFor(() => expect((switchFor("Game window") as HTMLButtonElement).disabled).toBe(false));

    const before = JSON.parse(JSON.stringify(current)) as LaunchOptions;
    switchFor("Game window").click();
    await waitFor(() => expect(mocks.setLaunchOptions).toHaveBeenCalledTimes(1));
    const first = mocks.setLaunchOptions.mock.calls[0][1] as LaunchOptions;
    expect(first.display.mode).toBe("gamescope");
    expect(changedFields(before, first).sort()).toEqual(["display.height", "display.mode", "display.width"]);
    await waitFor(() => expect((switchFor("Frame limit") as HTMLButtonElement).disabled).toBe(false));

    switchFor("Frame limit").click();
    await waitFor(() => expect(mocks.setLaunchOptions).toHaveBeenCalledTimes(2));
    const second = mocks.setLaunchOptions.mock.calls[1][1] as LaunchOptions;
    expect(changedFields(first, second)).toEqual(["display.refresh"]);
    expect(second.display.refresh).toBe(60);
    // Nothing else moved in the whole profile.
    expect(second.display.mode).toBe("gamescope");
    expect(second.env).toEqual({});
  });

  it("keeps the overlay opt-out in the overlay settings, not the profile", async () => {
    renderPanel();
    await screen.findByText("Ready to play");

    switchFor("In-game overlay").click();
    await waitFor(() => expect(mocks.setOverlayGameDisabled).toHaveBeenCalledWith(7, true));
    expect(mocks.setLaunchOptions).not.toHaveBeenCalled();
  });

  it("writes the audio device as one environment entry", async () => {
    renderPanel();
    await screen.findByText("Ready to play");

    const before = JSON.parse(JSON.stringify(current)) as LaunchOptions;
    switchFor("Audio device").click();
    const field = await screen.findByRole("textbox", { name: "Audio device" });
    fireEvent.change(field, { target: { value: "alsa_output.hdmi" } });

    const apply = await screen.findByRole("button", { name: "Use this device" });
    await waitFor(() => expect((apply as HTMLButtonElement).disabled).toBe(false));
    apply.click();

    await waitFor(() => expect(mocks.setLaunchOptions).toHaveBeenCalledTimes(1));
    const saved = mocks.setLaunchOptions.mock.calls[0][1] as LaunchOptions;
    expect(changedFields(before, saved)).toEqual(["env.PULSE_SINK"]);
    expect(saved.env.PULSE_SINK).toBe("alsa_output.hdmi");
  });

  it("keeps the raw panel as the only advanced surface", async () => {
    window.localStorage.setItem(ADVANCED_TIERS_STORAGE_KEY, "1");
    renderPanel({ isEditor: true });
    await screen.findByText("Ready to play");

    const advanced = await screen.findByText("Environment overrides");
    expect(advanced).toBeTruthy();
    // Tier 3 also owns the custom launch command for an editor (#143).
    expect(screen.getByText("Startup command")).toBeTruthy();
    // The tier-2 switches are still the primary surface, above the raw panel.
    const raw = screen.getByText("Wine prefix").closest("div");
    expect(raw).toBeTruthy();
    expect(within(document.body).getByText("Lock advanced view")).toBeTruthy();
  });

  it("hides the custom launch command until the viewer unlocks", async () => {
    renderPanel({ isEditor: true });
    await screen.findByText("Ready to play");

    expect(screen.queryByText("Startup command")).toBeNull();
    screen.getByText("Unlock advanced view").click();
    expect(await screen.findByText("Startup command")).toBeTruthy();
  });
});
