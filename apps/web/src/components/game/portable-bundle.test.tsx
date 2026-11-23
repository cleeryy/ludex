// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { InstalledGame, PortableBundleEstimate } from "@/lib/types";

/**
 * #332 render tests: the portable-bundle panel never offers a broken action.
 *
 * Not installed, still estimating, blocked by a missing executable, running and
 * done are the five states a user can meet; the copy and the runtime line are
 * the whole point of the screen.
 */

const mocks = vi.hoisted(() => ({
  portableEstimate: vi.fn(),
  createPortableBundle: vi.fn(),
  onPortableProgress: vi.fn(),
  pickFolder: vi.fn(),
  openPath: vi.fn(),
}));

vi.mock("@/lib/native", () => ({
  isNative: () => true,
  native: {
    onPortableProgress: mocks.onPortableProgress,
    pickFolder: mocks.pickFolder,
    openPath: mocks.openPath,
  },
}));

vi.mock("@/lib/install", () => ({
  installBridge: {
    portableEstimate: mocks.portableEstimate,
    createPortableBundle: mocks.createPortableBundle,
  },
}));

vi.mock("@/state/toasts", () => ({
  useToast: () => ({ push: () => undefined }),
}));

import { PortableBundlePanel } from "@/components/game/portable-bundle";

const INSTALLED: InstalledGame = {
  gameId: 7,
  versionId: 71,
  title: "Outer Wilds",
  version: "1.1.14",
  installPath: "/home/mara/Games/outer-wilds/1.1.14",
  executable: "/home/mara/Games/outer-wilds/1.1.14/bin/OuterWilds.exe",
  sizeBytes: 8_400_000_000,
  installedAt: "2026-11-20T10:00:00Z",
  lastLaunchedAt: null,
  source: "ludex",
};

const ESTIMATE: PortableBundleEstimate = {
  supported: true,
  platform: "linux",
  gameId: 7,
  bytes: 8_400_000_000,
  runtimeRequirement: "windows-compat",
  executablePresent: true,
  reason: null,
};

function wrapper(children: React.ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}

function renderPanel(installed: InstalledGame | undefined) {
  return render(wrapper(<PortableBundlePanel gameId={7} title="Outer Wilds" installed={installed} />));
}

beforeEach(() => {
  mocks.portableEstimate.mockReset();
  mocks.createPortableBundle.mockReset();
  mocks.onPortableProgress.mockReset();
  mocks.onPortableProgress.mockResolvedValue(() => undefined);
  mocks.pickFolder.mockReset();
  mocks.openPath.mockReset();
});

afterEach(cleanup);

describe("PortableBundlePanel", () => {
  it("asks for an install instead of offering a bundle", async () => {
    mocks.portableEstimate.mockResolvedValue(ESTIMATE);
    renderPanel(undefined);
    expect(screen.getByText("Portable Linux bundle")).toBeTruthy();
    expect(screen.getByText("Install the game on this machine first.")).toBeTruthy();
    const button = screen.getByRole("button", { name: "Create portable Linux bundle" });
    expect(button.hasAttribute("disabled")).toBe(true);
    // No estimate is requested without an install.
    expect(mocks.portableEstimate).not.toHaveBeenCalled();
  });

  it("shows the estimate while it is being computed", async () => {
    mocks.portableEstimate.mockImplementation(() => new Promise(() => undefined));
    renderPanel(INSTALLED);
    expect(await screen.findByText("Estimating the bundle size…")).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Create portable Linux bundle" }).hasAttribute("disabled"),
    ).toBe(true);
  });

  it("renders nothing when the desktop does not support portable bundles", async () => {
    mocks.portableEstimate.mockResolvedValue({ ...ESTIMATE, supported: false, reason: "Linux only" });
    const { container } = renderPanel(INSTALLED);
    await waitFor(() => expect(container.textContent).toBe(""));
  });

  it("blocks the action with the reason the backend gave", async () => {
    mocks.portableEstimate.mockResolvedValue({
      ...ESTIMATE,
      executablePresent: false,
      reason: "The recorded executable is missing.",
    });
    renderPanel(INSTALLED);
    expect(await screen.findByText("The recorded executable is missing.")).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Create portable Linux bundle" }).hasAttribute("disabled"),
    ).toBe(true);
  });

  it("creates in place, showing progress then the created path", async () => {
    mocks.portableEstimate.mockResolvedValue(ESTIMATE);
    let resolveCreate: (value: unknown) => void = () => undefined;
    mocks.createPortableBundle.mockImplementation(
      () =>
        new Promise((resolve) => {
          resolveCreate = resolve;
        }),
    );

    renderPanel(INSTALLED);
    // Wait for the estimate: until it is here the action is deliberately
    // disabled, so a click before it would be a no-op.
    await screen.findByText("Estimated size");
    fireEvent.click(screen.getByRole("button", { name: "Create portable Linux bundle" }));

    // The dialog repeats the size and the runtime the target still needs.
    expect(await screen.findByText("Create a portable bundle of Outer Wilds")).toBeTruthy();
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByText("Needed on the target machine (UMU, Proton or Wine)")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Create in the install folder" }));
    await waitFor(() => expect(mocks.createPortableBundle).toHaveBeenCalledWith(7, null));
    expect(await screen.findByText("Preparing the bundle…")).toBeTruthy();

    resolveCreate({
      path: INSTALLED.installPath,
      bytes: 24_000,
      runtimeRequired: "windows-compat",
      fileCount: 4,
      replaced: ["run.sh"],
      copied: false,
    });
    expect(await screen.findByText("Portable bundle ready")).toBeTruthy();
    expect(screen.getByText(INSTALLED.installPath)).toBeTruthy();
    // An archive-shipped script was replaced and the user is told.
    expect(screen.getByText(/Replaced files shipped with the archive: run\.sh/)).toBeTruthy();
    // The Windows caveat stays visible after success.
    expect(screen.getByText(/the target machine still needs UMU, Proton or Wine/)).toBeTruthy();
  });

  it("copies to the picked destination and reports a native game as self-contained", async () => {
    mocks.portableEstimate.mockResolvedValue({
      ...ESTIMATE,
      runtimeRequirement: "linux-native",
    });
    mocks.createPortableBundle.mockResolvedValue({
      path: "/media/usb/Games/1.1.14",
      bytes: 24_000,
      runtimeRequired: "linux-native",
      fileCount: 4,
      replaced: [],
      copied: true,
    });
    mocks.pickFolder.mockResolvedValue("/media/usb/Games");

    renderPanel(INSTALLED);
    await screen.findByText("Estimated size");
    fireEvent.click(screen.getByRole("button", { name: "Create portable Linux bundle" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("None: native Linux build")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Browse" }));
    await waitFor(() => expect(mocks.pickFolder).toHaveBeenCalled());
    await waitFor(() =>
      expect(screen.getByText(/A folder named 1\.1\.14 is created inside the folder you pick/)).toBeTruthy(),
    );

    fireEvent.click(screen.getByRole("button", { name: "Copy and create bundle" }));
    await waitFor(() =>
      expect(mocks.createPortableBundle).toHaveBeenCalledWith(7, "/media/usb/Games"),
    );
    expect(await screen.findByText("/media/usb/Games/1.1.14")).toBeTruthy();
    expect(screen.queryByText(/the target machine still needs UMU/)).toBeNull();
  });

  it("shows the backend error and stays on the form", async () => {
    mocks.portableEstimate.mockResolvedValue(ESTIMATE);
    mocks.createPortableBundle.mockRejectedValue(new Error("/media/usb/Games/1.1.14 already exists."));

    renderPanel(INSTALLED);
    await screen.findByText("Estimated size");
    fireEvent.click(screen.getByRole("button", { name: "Create portable Linux bundle" }));
    fireEvent.click(await screen.findByRole("button", { name: "Create in the install folder" }));

    expect(await screen.findByRole("alert")).toHaveProperty(
      "textContent",
      "/media/usb/Games/1.1.14 already exists.",
    );
    expect(screen.getByRole("button", { name: "Create in the install folder" })).toBeTruthy();
  });
});
