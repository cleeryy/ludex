// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// The timeout only exists on the real HTTP path; the mock adapter bypasses it.
vi.mock("@/lib/flags", () => ({ USING_MOCKS: false }));

import { api, ApiError, formRequest, publicRequest, tokenStore, UNAUTHORIZED_EVENT, REQUEST_TIMEOUT_MS } from "@/lib/api";

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

describe("api adapter boundaries (issue 237)", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    tokenStore.clear();
  });

  it("never sends the session token on a public route and survives its 401", async () => {
    tokenStore.set("session-token");
    const unauthorized = vi.fn();
    window.addEventListener(UNAUTHORIZED_EVENT, unauthorized);
    const fetchMock = vi.fn(async (_url: string, _init?: RequestInit) =>
      new Response(JSON.stringify({ error: { code: "invalid_password", message: "Wrong password" } }), {
        status: 401,
        headers: { "Content-Type": "application/json" },
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await expect(publicRequest("/share/token")).rejects.toMatchObject({
      status: 401,
      code: "invalid_password",
    });

    const init = fetchMock.mock.calls[0]?.[1] as RequestInit | undefined;
    expect((init?.headers as Record<string, string> | undefined)?.Authorization).toBeUndefined();
    expect(tokenStore.get()).toBe("session-token");
    expect(unauthorized).not.toHaveBeenCalled();
    window.removeEventListener(UNAUTHORIZED_EVENT, unauthorized);
  });

  it("sends multipart bytes without claiming a JSON body", async () => {
    tokenStore.set("session-token");
    const fetchMock = vi.fn(async (_url: string, _init?: RequestInit) => new Response(JSON.stringify({ ok: true }), { status: 200 }));
    vi.stubGlobal("fetch", fetchMock);
    const form = new FormData();
    form.set("file", new Blob(["x"]), "cover.png");

    await formRequest("/games/1/artwork", form);

    const init = fetchMock.mock.calls[0]?.[1] as RequestInit | undefined;
    const headers = init?.headers as Record<string, string> | undefined;
    expect(init?.body).toBe(form);
    expect(headers?.["Content-Type"]).toBeUndefined();
    expect(headers?.Authorization).toBe("Bearer session-token");
  });
});
