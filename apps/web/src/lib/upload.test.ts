import { describe, expect, it } from "vitest";
import {
  UPLOAD_STORAGE_KEY,
  UploadController,
  chunkRange,
  isArchiveName,
  pendingParts,
  readStoredUpload,
  uploadIndicator,
  writeStoredUpload,
  type StoredUpload,
  type UploadSnapshot,
  type UploadSource,
  type UploadStorage,
  type UploadTransport,
} from "@/lib/upload";
import type { CompleteUploadResponse, UploadSession } from "@/lib/types";

/**
 * #185: the chunk loop lives in the controller, not in a screen component, so
 * these tests exercise it without React at all — which is exactly the property
 * that keeps a route change from interrupting a transfer.
 */

class MemoryStorage implements UploadStorage {
  private map = new Map<string, string>();
  getItem(key: string) {
    return this.map.get(key) ?? null;
  }
  setItem(key: string, value: string) {
    this.map.set(key, value);
  }
  removeItem(key: string) {
    this.map.delete(key);
  }
  has(key: string) {
    return this.map.has(key);
  }
}

async function flush(times = 10): Promise<void> {
  for (let index = 0; index < times; index += 1) await Promise.resolve();
}

function makeSession(overrides: Partial<UploadSession> = {}): UploadSession {
  return {
    id: "u1",
    fileName: "game.zip",
    size: 16,
    sha256: null,
    chunkSize: 4,
    totalParts: 4,
    receivedBytes: 0,
    receivedParts: [],
    status: "open",
    createdAt: "2026-11-12T10:00:00Z",
    updatedAt: "2026-11-12T10:00:00Z",
    gameId: null,
    versionId: null,
    error: null,
    ...overrides,
  };
}

function makeSource(name = "game.zip", size = 16): UploadSource {
  return {
    name,
    size,
    slice(start: number, end: number) {
      return new Blob([new Uint8Array(Math.max(0, end - start))]);
    },
  };
}

const resultFor = (session: UploadSession): CompleteUploadResponse => ({
  id: session.id,
  gameId: 42,
  versionId: 7,
  fileName: session.fileName,
  size: session.size,
  sha256: null,
});

/**
 * Minimal fake server: parts resolve immediately unless the test holds them,
 * so a test can stop the loop mid-transfer, inspect state, and resume.
 */
function fakeTransport(initial: UploadSession | null = null) {
  const calls = {
    init: [] as Array<{ fileName: string; size: number }>,
    gets: [] as string[],
    parts: [] as number[],
    completes: [] as string[],
    cancels: [] as string[],
  };
  const state = {
    session: initial ? { ...initial, receivedParts: [...initial.receivedParts] } : null,
    hold: false,
    pending: [] as Array<{ resolve: () => void; reject: (cause: unknown) => void }>,
    failure: null as unknown,
  };

  const commit = (part: number, chunk: Blob, onProgress?: (loaded: number) => void) => {
    const session = state.session;
    if (!session) throw new Error("upload not found");
    if (state.failure) throw state.failure;
    if (!session.receivedParts.includes(part)) {
      session.receivedParts = [...session.receivedParts, part].sort((left, right) => left - right);
      session.receivedBytes += chunk.size;
    }
    onProgress?.(chunk.size);
    return {
      id: session.id,
      part,
      receivedBytes: session.receivedBytes,
      receivedParts: session.receivedParts.length,
      totalParts: session.totalParts,
    };
  };

  const transport: UploadTransport = {
    async initUpload(body) {
      calls.init.push(body);
      state.session = makeSession({ fileName: body.fileName, size: body.size });
      return { ...state.session, receivedParts: [...state.session.receivedParts] };
    },
    async getUpload(id) {
      calls.gets.push(id);
      if (!state.session) throw new Error("upload not found");
      return { ...state.session, receivedParts: [...state.session.receivedParts] };
    },
    uploadPart(_id, part, chunk, onProgress) {
      calls.parts.push(part);
      if (!state.hold) {
        return Promise.resolve(commit(part, chunk, onProgress));
      }
      return new Promise((resolve, reject) => {
        state.pending.push({
          resolve: () => {
            try {
              resolve(commit(part, chunk, onProgress));
            } catch (cause) {
              reject(cause);
            }
          },
          reject,
        });
      });
    },
    async completeUpload(id) {
      calls.completes.push(id);
      if (!state.session) throw new Error("upload not found");
      return resultFor(state.session);
    },
    async cancelUpload(id) {
      calls.cancels.push(id);
      state.session = null;
    },
  };

  return {
    transport,
    calls,
    state,
    releaseNext() {
      state.pending.shift()?.resolve();
    },
    releaseAll() {
      state.hold = false;
      while (state.pending.length > 0) state.pending.shift()?.resolve();
    },
  };
}

describe("upload helpers", () => {
  it("accepts only the archive types the server indexes", () => {
    expect(isArchiveName("game.ZIP")).toBe(true);
    expect(isArchiveName("game.rar")).toBe(true);
    expect(isArchiveName("game.7z")).toBe(true);
    expect(isArchiveName("game.tar.gz")).toBe(false);
    expect(isArchiveName("setup.exe")).toBe(false);
  });

  it("lists the parts the server still needs, in order", () => {
    expect(pendingParts({ receivedParts: [0, 2], totalParts: 4 })).toEqual([1, 3]);
    expect(pendingParts({ receivedParts: [], totalParts: 2 })).toEqual([0, 1]);
    expect(pendingParts({ receivedParts: [0, 1], totalParts: 2 })).toEqual([]);
  });

  it("maps a part to its byte range and clamps the final chunk", () => {
    const session = { chunkSize: 4, size: 10 };
    expect(chunkRange(session, 0)).toEqual({ start: 0, end: 4 });
    expect(chunkRange(session, 2)).toEqual({ start: 8, end: 10 });
  });

  it("round-trips the stored session and ignores malformed values", () => {
    const storage = new MemoryStorage();
    const stored: StoredUpload = { id: "u1", fileName: "game.zip", size: 16 };
    writeStoredUpload(storage, stored);
    expect(readStoredUpload(storage)).toEqual(stored);

    storage.setItem(UPLOAD_STORAGE_KEY, "not json");
    expect(readStoredUpload(storage)).toBeNull();
    storage.setItem(UPLOAD_STORAGE_KEY, JSON.stringify({ id: "", fileName: "x.zip", size: 4 }));
    expect(readStoredUpload(storage)).toBeNull();
    storage.setItem(UPLOAD_STORAGE_KEY, JSON.stringify({ id: "u1", fileName: "x.zip", size: -1 }));
    expect(readStoredUpload(storage)).toBeNull();

    writeStoredUpload(storage, null);
    expect(storage.has(UPLOAD_STORAGE_KEY)).toBe(false);
  });
});

describe("UploadController", () => {
  it("keeps the transfer running when the screen unmounts and shows it again on remount", async () => {
    const server = fakeTransport();
    server.state.hold = true;
    const completed: CompleteUploadResponse[] = [];
    const controller = new UploadController({
      transport: server.transport,
      storage: new MemoryStorage(),
      onComplete: (result) => completed.push(result),
    });

    controller.select(makeSource());
    const starting = controller.start();
    await flush();

    // The Upload screen is mounted and the first chunk is in flight.
    expect(server.calls.parts).toEqual([0]);
    const seen: UploadSnapshot[] = [];
    const unmount = controller.subscribe(() => seen.push(controller.getSnapshot()));

    // Navigating away unmounts the screen (its subscription goes away)...
    unmount();
    server.releaseNext();
    await flush();
    expect(server.calls.parts).toEqual([0, 1]);

    // ...and coming back reads the same live state, no reselect required.
    const remount = controller.subscribe(() => seen.push(controller.getSnapshot()));
    const live = controller.getSnapshot();
    expect(live.phase).toBe("uploading");
    expect(live.file?.name).toBe("game.zip");
    expect(controller.getSnapshot().uploaded).toBe(4);

    server.releaseAll();
    await starting;
    await flush();
    remount();

    const done = controller.getSnapshot();
    expect(done.phase).toBe("complete");
    expect(done.result?.gameId).toBe(42);
    expect(completed).toHaveLength(1);
    expect(server.calls.parts).toEqual([0, 1, 2, 3]);
    expect(seen.some((snapshot) => snapshot.percent > 0 && snapshot.percent < 100)).toBe(true);
  });

  it("stops the loop and deletes the server session on cancel", async () => {
    const storage = new MemoryStorage();
    const server = fakeTransport();
    server.state.hold = true;
    const controller = new UploadController({ transport: server.transport, storage });

    controller.select(makeSource());
    const starting = controller.start();
    await flush();
    expect(server.calls.parts).toEqual([0]);
    expect(storage.has(UPLOAD_STORAGE_KEY)).toBe(true);

    const cancelling = controller.cancel();
    await flush();
    // The in-flight chunk settles before DELETE, so no PUT can recreate it.
    expect(server.calls.cancels).toEqual([]);

    server.releaseNext();
    await cancelling;
    await starting;

    expect(server.calls.cancels).toEqual(["u1"]);
    expect(server.calls.parts).toEqual([0]);
    expect(storage.has(UPLOAD_STORAGE_KEY)).toBe(false);
    const snapshot = controller.getSnapshot();
    expect(snapshot.phase).toBe("idle");
    expect(snapshot.session).toBeNull();
    expect(snapshot.file).toBeNull();
    expect(snapshot.uploaded).toBe(0);
  });

  it("restores a persisted session after a reload and resumes from receivedParts", async () => {
    const storage = new MemoryStorage();
    writeStoredUpload(storage, { id: "u9", fileName: "game.zip", size: 16 });
    const server = fakeTransport(
      makeSession({ id: "u9", receivedParts: [0, 2], receivedBytes: 8 }),
    );
    const controller = new UploadController({ transport: server.transport, storage });

    await controller.restore();
    const restored = controller.getSnapshot();
    expect(server.calls.gets).toEqual(["u9"]);
    expect(restored.ready).toBe(true);
    expect(restored.awaitingFile).toBe(true);
    expect(restored.session?.fileName).toBe("game.zip");
    expect(restored.uploaded).toBe(8);
    expect(restored.percent).toBe(50);
    // The File cannot survive a reload: nothing is sent until it is picked.
    expect(server.calls.parts).toEqual([]);

    expect(controller.select(makeSource())).toBe("selected");
    expect(controller.getSnapshot().awaitingFile).toBe(false);
    await controller.start();

    expect(server.calls.parts).toEqual([1, 3]);
    expect(controller.getSnapshot().phase).toBe("complete");
    expect(storage.has(UPLOAD_STORAGE_KEY)).toBe(false);
  });

  it("blocks a second start while one upload is running", async () => {
    const server = fakeTransport();
    server.state.hold = true;
    const controller = new UploadController({ transport: server.transport, storage: new MemoryStorage() });

    controller.select(makeSource());
    const first = controller.start();
    await flush();

    const second = await controller.start();
    expect(second).toBe("busy");
    expect(controller.select(makeSource("other.zip", 16))).toBe("busy");
    expect(server.calls.init).toHaveLength(1);
    expect(server.calls.parts).toEqual([0]);

    server.releaseAll();
    await first;
  });

  it("refuses a different archive while a server session is open", async () => {
    const storage = new MemoryStorage();
    writeStoredUpload(storage, { id: "u1", fileName: "game.zip", size: 16 });
    const server = fakeTransport(makeSession({ fileName: "game.zip", size: 16 }));
    const controller = new UploadController({ transport: server.transport, storage });
    await controller.restore();

    expect(controller.select(makeSource("other.zip", 16))).toBe("mismatch");
    expect(controller.select(makeSource("game.zip", 32))).toBe("mismatch");
    expect(controller.select(makeSource("notes.txt", 4))).toBe("invalid-type");
    expect(controller.select(makeSource())).toBe("selected");
  });

  it("keeps the session resumable after a failed chunk", async () => {
    const storage = new MemoryStorage();
    const server = fakeTransport();
    server.state.failure = new Error("network down");
    const errors: string[] = [];
    const controller = new UploadController({
      transport: server.transport,
      storage,
      onError: (message) => errors.push(message),
    });

    controller.select(makeSource());
    await controller.start();

    const failed = controller.getSnapshot();
    expect(failed.phase).toBe("error");
    expect(failed.error).toBe("network down");
    expect(errors).toEqual(["network down"]);
    // The id stays so a reload can offer the Resume card.
    expect(readStoredUpload(storage)?.id).toBe("u1");

    server.state.failure = null;
    await controller.cancel();
    expect(server.calls.cancels).toEqual(["u1"]);
  });

  it("deletes a session whose init finished after the cancel", async () => {
    const storage = new MemoryStorage();
    const server = fakeTransport();
    let release = () => {};
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    const transport: UploadTransport = {
      ...server.transport,
      initUpload: async (body) => {
        await gate;
        return server.transport.initUpload(body);
      },
    };
    const controller = new UploadController({ transport, storage });

    controller.select(makeSource());
    const starting = controller.start();
    await Promise.resolve();
    const cancelling = controller.cancel();
    release();
    await cancelling;
    await starting;

    // The id arrived after the user cancelled: remove the empty session.
    expect(server.calls.cancels).toEqual(["u1"]);
    expect(server.calls.parts).toEqual([]);
    expect(storage.has(UPLOAD_STORAGE_KEY)).toBe(false);
    expect(controller.getSnapshot().phase).toBe("idle");
  });

  it("reports nothing to the shell before a transfer starts", () => {    const idle: UploadSnapshot = {
      ready: true,
      phase: "idle",
      file: null,
      session: null,
      uploaded: 0,
      total: 0,
      percent: 0,
      error: null,
      result: null,
      awaitingFile: false,
      active: false,
    };
    expect(uploadIndicator(idle)).toBeNull();

    const uploading: UploadSnapshot = {
      ...idle,
      phase: "uploading",
      file: { name: "game.zip", size: 100 },
      uploaded: 42,
      total: 100,
      percent: 42,
      active: true,
    };
    expect(uploadIndicator(uploading)).toEqual({
      phase: "uploading",
      fileName: "game.zip",
      percent: 42,
    });

    expect(uploadIndicator({ ...uploading, phase: "verifying" })).toEqual({
      phase: "verifying",
      fileName: "game.zip",
      percent: 100,
    });
    expect(uploadIndicator({ ...uploading, phase: "complete", session: null, file: null })).toBeNull();
    expect(uploadIndicator({ ...uploading, phase: "error" })).toBeNull();
    expect(uploadIndicator({ ...uploading, phase: "idle", awaitingFile: true, file: null })).toBeNull();
  });
});
