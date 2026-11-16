// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// The timeout only exists on the real HTTP path; the mock adapter bypasses it.
vi.mock("@/lib/flags", () => ({ USING_MOCKS: false }));

import { api, ApiError, REQUEST_TIMEOUT_MS } from "@/lib/api";

/** A fetch that accepts the connection and never answers until aborted. */
function hangingFetch(): ReturnType<typeof vi.fn> {
  return vi.fn((_url: string, init?: RequestInit) => {
    return new Promise<Response>((_resolve, reject) => {
      init?.signal?.addEventListener("abort", () => {
        reject(new DOMException("The operation was aborted.", "AbortError"));
      });
    });
  });
}

describe("api request timeout (issue 237)", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("rejects with a timeout ApiError when the server never answers", async () => {
    vi.stubGlobal("fetch", hangingFetch());

    const request = api.health();
    const settled = request.then(
      () => null,
      (error: unknown) => error,
    );
    await vi.advanceTimersByTimeAsync(REQUEST_TIMEOUT_MS + 1);
    const error = await settled;

    expect(error).toBeInstanceOf(ApiError);
    expect(error).toMatchObject({ status: 0, code: "timeout" });
  });

  it("keeps the request pending until the deadline, then aborts exactly once", async () => {
    const fetchMock = hangingFetch();
    vi.stubGlobal("fetch", fetchMock);

    const settled = api.health().then(
      () => null,
      (error: unknown) => error,
    );

    await vi.advanceTimersByTimeAsync(REQUEST_TIMEOUT_MS - 1);
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const signal = (fetchMock.mock.calls[0]?.[1] as RequestInit | undefined)?.signal;
    expect(signal?.aborted).toBe(false);

    await vi.advanceTimersByTimeAsync(1);
    await settled;
    expect(signal?.aborted).toBe(true);
  });
});
