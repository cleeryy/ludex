// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ExtractStatus } from "@/lib/types";

const mocks = vi.hoisted(() => ({
  extractVersion: vi.fn(),
  push: vi.fn(),
}));

vi.mock("@/lib/api", () => ({
  api: {
    extractVersion: (...args: unknown[]) => mocks.extractVersion(...args),
  },
}));

vi.mock("@/state/toasts", () => ({
  useToast: () => ({ push: mocks.push }),
}));

import { PrepareTransport } from "@/components/admin/prepare-transport";

beforeEach(() => {
  mocks.extractVersion.mockReset();
  mocks.push.mockReset();
});

afterEach(() => {
  cleanup();
});

function status(patch: Partial<ExtractStatus> = {}): ExtractStatus {
  return {
    versionId: 70,
    archiveSha256: "a".repeat(64),
    status: "queued",
    files: 0,
    bytes: 0,
    error: null,
    ...patch,
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

function renderAction(props: Partial<Parameters<typeof PrepareTransport>[0]> = {}) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={client}>
      <PrepareTransport
        versionId={70}
        archiveFormat="7z"
        cacheEnabled
        pollMs={10}
        {...props}
      />
    </QueryClientProvider>,
  );
}

describe("prepare file transport", () => {
  it("polls the extract route and renders the progress until ready", async () => {
    const first = deferred<ExtractStatus>();
    const second = deferred<ExtractStatus>();
    mocks.extractVersion
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise);
    const onReady = vi.fn();

    renderAction({ onReady });
    fireEvent.click(screen.getByRole("button", { name: "Prepare file transport" }));

    expect(screen.getByText("Preparing")).toBeTruthy();
    await waitFor(() => expect(mocks.extractVersion).toHaveBeenCalledTimes(1));

    await act(async () => {
      first.resolve(status({ status: "extracting", files: 120, bytes: 3 * 1024 ** 2 }));
      await first.promise;
    });
    expect(await screen.findByText("Extracting")).toBeTruthy();
    expect(screen.getByText(/120 files/)).toBeTruthy();
    expect(screen.getByRole("progressbar", { name: "Extraction progress" })).toBeTruthy();

    // The active status keeps the poll alive; the next call returns the entry.
    await waitFor(() => expect(mocks.extractVersion).toHaveBeenCalledTimes(2));
    await act(async () => {
      second.resolve(status({ status: "ready", files: 1_240, bytes: 4_509_715_660 }));
      await second.promise;
    });

    expect(await screen.findByText("Ready")).toBeTruthy();
    expect(screen.getByText(/1,240 files/)).toBeTruthy();
    expect(onReady).toHaveBeenCalledTimes(1);
    expect(mocks.push).toHaveBeenCalledWith(
      expect.objectContaining({ title: "File transport ready", tone: "success" }),
    );
  });

  it("shows the server's failure detail and offers a retry", async () => {
    mocks.extractVersion
      .mockResolvedValueOnce(status({ status: "extracting", files: 10, bytes: 2_048 }))
      .mockResolvedValueOnce(
        status({ status: "failed", error: "The archive is encrypted" }),
      );

    renderAction();
    fireEvent.click(screen.getByRole("button", { name: "Prepare file transport" }));

    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toBe("The archive is encrypted");
    expect(screen.getByRole("button", { name: "Try again" })).toBeTruthy();
    expect(mocks.push).not.toHaveBeenCalledWith(
      expect.objectContaining({ title: "File transport ready" }),
    );
  });

  it("waits while the cache state is unknown", () => {
    renderAction({ cacheKnown: false });
    const button = screen.getByRole("button", { name: "Prepare file transport" }) as HTMLButtonElement;
    expect(button.disabled).toBe(true);
    expect(button.getAttribute("title")).toBe("Checking the extracted cache");
  });

  it("is disabled while the cache is off", () => {
    renderAction({ cacheEnabled: false });
    const button = screen.getByRole("button", { name: "Prepare file transport" }) as HTMLButtonElement;
    expect(button.disabled).toBe(true);
    expect(button.getAttribute("title")).toBe(
      "The extracted cache is off. Turn it on to prepare archives.",
    );
  });

  it("is disabled when the archive format is unknown", () => {
    renderAction({ archiveFormat: null });
    const button = screen.getByRole("button", { name: "Prepare file transport" }) as HTMLButtonElement;
    expect(button.disabled).toBe(true);
    expect(button.getAttribute("title")).toBe("The archive format is unknown. Run a scan first.");
  });
});
