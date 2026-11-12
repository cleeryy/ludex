import { errorMessage, isApiError } from "@/lib/http";
import type { CompleteUploadResponse, UploadPartResponse, UploadSession } from "@/lib/types";

/**
 * Resumable archive uploads (#185).
 *
 * The transfer loop lives in `UploadController`, not in the Upload screen: a
 * route change unmounts the screen but not the provider above the router, so
 * the chunk loop keeps running and the shell can read live progress. The
 * controller is framework-free on purpose, which also keeps it testable in the
 * plain vitest environment this app uses (no DOM).
 */

/** Storage key kept from the screen-owned implementation; older sessions resume. */
export const UPLOAD_STORAGE_KEY = "ludex.upload";

/** The archive types the server accepts (`is_archive_path`). */
export const UPLOAD_EXTENSIONS = [".zip", ".rar", ".7z"] as const;

export type UploadPhase = "idle" | "uploading" | "verifying" | "complete" | "error";

export interface StoredUpload {
  id: string;
  fileName: string;
  size: number;
}

/** `File` satisfies this; tests use a tiny fake without a DOM. */
export interface UploadSource {
  name: string;
  size: number;
  slice(start: number, end: number): Blob;
}

export interface UploadTransport {
  initUpload(input: { fileName: string; size: number }): Promise<UploadSession>;
  getUpload(id: string): Promise<UploadSession>;
  uploadPart(
    id: string,
    part: number,
    chunk: Blob,
    onProgress?: (loaded: number) => void,
  ): Promise<UploadPartResponse>;
  completeUpload(id: string): Promise<CompleteUploadResponse>;
  cancelUpload(id: string): Promise<void>;
}

export interface UploadStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

export interface UploadSnapshot {
  /** False until a persisted session has been checked (or discarded). */
  ready: boolean;
  phase: UploadPhase;
  file: { name: string; size: number } | null;
  session: UploadSession | null;
  uploaded: number;
  total: number;
  percent: number;
  error: string | null;
  result: CompleteUploadResponse | null;
  /** A server session was restored but the File cannot survive a reload. */
  awaitingFile: boolean;
  /** The chunk loop is moving chunks or waiting for `complete`. */
  active: boolean;
}

export type SelectResult = "selected" | "invalid-type" | "mismatch" | "busy";
export type StartResult = "started" | "busy" | "no-file";

const EMPTY: UploadSnapshot = {
  ready: false,
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

export function isArchiveName(name: string): boolean {
  return UPLOAD_EXTENSIONS.some((extension) => name.toLowerCase().endsWith(extension));
}

export function pendingParts(
  session: Pick<UploadSession, "receivedParts" | "totalParts">,
): number[] {
  const received = new Set(session.receivedParts);
  const parts: number[] = [];
  for (let part = 0; part < session.totalParts; part += 1) {
    if (!received.has(part)) parts.push(part);
  }
  return parts;
}

export function chunkRange(
  session: Pick<UploadSession, "chunkSize" | "size">,
  part: number,
): { start: number; end: number } {
  const start = part * session.chunkSize;
  return { start, end: Math.min(start + session.chunkSize, session.size) };
}

export function readStoredUpload(
  storage: UploadStorage | null,
  key = UPLOAD_STORAGE_KEY,
): StoredUpload | null {
  if (!storage) return null;
  try {
    const raw = storage.getItem(key);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Partial<StoredUpload> | null;
    if (
      !parsed ||
      typeof parsed.id !== "string" ||
      !parsed.id ||
      typeof parsed.fileName !== "string" ||
      !parsed.fileName ||
      typeof parsed.size !== "number" ||
      !Number.isFinite(parsed.size) ||
      parsed.size <= 0
    ) {
      return null;
    }
    return { id: parsed.id, fileName: parsed.fileName, size: parsed.size };
  } catch {
    return null;
  }
}

export function writeStoredUpload(
  storage: UploadStorage | null,
  value: StoredUpload | null,
  key = UPLOAD_STORAGE_KEY,
): void {
  if (!storage) return;
  try {
    if (value) storage.setItem(key, JSON.stringify(value));
    else storage.removeItem(key);
  } catch {
    /* private mode: the session stays resumable server-side, just not listed */
  }
}

export interface UploadIndicatorState {
  phase: "uploading" | "verifying";
  fileName: string;
  percent: number;
}

/**
 * What the shell indicator shows. Only a running transfer is visible: a
 * restored-but-idle session lives on the Upload screen as a Resume card, and a
 * finished or failed one is announced by its toast.
 */
export function uploadIndicator(snapshot: UploadSnapshot): UploadIndicatorState | null {
  if (snapshot.phase !== "uploading" && snapshot.phase !== "verifying") return null;
  return {
    phase: snapshot.phase,
    fileName: snapshot.file?.name ?? snapshot.session?.fileName ?? "",
    percent: snapshot.phase === "verifying" ? 100 : snapshot.percent,
  };
}

interface UploadControllerOptions {
  transport: UploadTransport;
  storage?: UploadStorage | null;
  key?: string;
  /** Called once per successful upload, on whatever route is mounted. */
  onComplete?: (result: CompleteUploadResponse) => void;
  /** Called once per failed upload; the session stays resumable. */
  onError?: (message: string) => void;
}

/**
 * Owns one upload at a time. Everything the UI needs is a plain snapshot, so
 * React (or a test) can subscribe without the loop holding a component alive.
 */
export class UploadController {
  private readonly options: UploadControllerOptions;
  private readonly listeners = new Set<() => void>();
  private snapshot: UploadSnapshot = EMPTY;
  private file: UploadSource | null = null;
  private restored = false;
  private running = false;
  private cancelled = false;
  private task: Promise<void> | null = null;

  constructor(options: UploadControllerOptions) {
    this.options = options;
  }

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  getSnapshot = (): UploadSnapshot => this.snapshot;

  /** True while a chunk loop (or the final `complete` call) is in flight. */
  get busy(): boolean {
    return this.running;
  }

  /** Check the persisted session id once; safe to call again after a remount. */
  async restore(): Promise<void> {
    if (this.restored) return;
    this.restored = true;
    const stored = readStoredUpload(this.storage);
    if (!stored) {
      this.update({ ready: true });
      return;
    }
    try {
      const session = await this.options.transport.getUpload(stored.id);
      if (session.status === "open") {
        this.update({
          ready: true,
          session,
          uploaded: session.receivedBytes,
          total: session.size,
          awaitingFile: true,
          file: null,
          phase: "idle",
        });
      } else {
        this.clearStored();
        this.update({ ready: true });
      }
    } catch (cause) {
      // A 404 means the server forgot it; anything else may be a blip, and the
      // id is worth keeping for the next reload.
      if (isApiError(cause) && cause.status === 404) this.clearStored();
      this.update({ ready: true });
    }
  }

  /** Stop locally without touching the server (sign-out, role change). */
  async forget(): Promise<void> {
    this.cancelled = true;
    const task = this.task;
    if (task) await task;
    this.cancelled = false;
    this.clearStored();
    this.file = null;
    this.update({ ...EMPTY, ready: true });
  }

  select(file: UploadSource): SelectResult {
    if (this.running) return "busy";
    if (!isArchiveName(file.name)) return "invalid-type";
    const session = this.snapshot.session;
    if (session && (file.name !== session.fileName || file.size !== session.size)) return "mismatch";
    this.file = file;
    this.update({
      phase: "idle",
      file: { name: file.name, size: file.size },
      uploaded: session?.receivedBytes ?? 0,
      total: session?.size ?? file.size,
      error: null,
      result: null,
      awaitingFile: false,
    });
    return "selected";
  }

  /**
   * Start or resume the selected file. The returned promise settles when the
   * transfer stops, which is what `cancel` awaits before deleting the session.
   */
  async start(): Promise<StartResult> {
    if (this.running) return "busy";
    const file = this.file;
    if (!file) return "no-file";

    this.running = true;
    this.cancelled = false;
    this.update({ phase: "uploading", error: null, result: null, awaitingFile: false });
    this.task = this.transfer(file);
    try {
      await this.task;
    } finally {
      this.task = null;
      this.running = false;
    }
    return "started";
  }

  /**
   * Stop the loop, delete the server session, then clear local state. Waiting
   * for the in-flight chunk before `DELETE` keeps a late `PUT` from recreating
   * a chunk after its session was removed.
   */
  async cancel(): Promise<void> {
    // Once the server assembles and verifies the archive there is nothing left
    // to stop: let `complete` finish and report the indexed game.
    if (this.snapshot.phase === "verifying") return;
    this.cancelled = true;
    const task = this.task;
    if (task) await task;

    // Read the id after the loop stops: a chunk or `init` that settled during
    // the wait may be the only record of a session created after the click.
    const sessionId = this.snapshot.session?.id ?? readStoredUpload(this.storage)?.id ?? null;
    this.file = null;
    this.update({ ...EMPTY, ready: true });
    this.clearStored();

    if (sessionId) {
      try {
        await this.options.transport.cancelUpload(sessionId);
      } catch {
        // Already gone, or the browser is offline; the TTL sweep is the backstop.
      }
    }
    this.cancelled = false;
  }

  dismissResult(): void {
    if (this.running) return;
    this.file = null;
    this.update({ ...EMPTY, ready: this.snapshot.ready });
  }

  private get storage(): UploadStorage | null {
    return this.options.storage ?? null;
  }

  private get key(): string {
    return this.options.key ?? UPLOAD_STORAGE_KEY;
  }

  private store(): void {
    const session = this.snapshot.session;
    const file = this.file;
    if (!session || !file) return;
    writeStoredUpload(this.storage, { id: session.id, fileName: file.name, size: file.size }, this.key);
  }

  private clearStored(): void {
    writeStoredUpload(this.storage, null, this.key);
  }

  private update(patch: Partial<UploadSnapshot>): void {
    const merged = { ...this.snapshot, ...patch };
    const active = merged.phase === "uploading" || merged.phase === "verifying";
    const percent =
      merged.phase === "complete"
        ? 100
        : merged.total > 0
          ? Math.min(100, Math.max(0, (merged.uploaded / merged.total) * 100))
          : 0;
    this.snapshot = { ...merged, active, percent };
    for (const listener of this.listeners) listener();
  }

  private async transfer(file: UploadSource): Promise<void> {
    try {
      const restored = this.snapshot.session;
      let session: UploadSession;
      if (restored) {
        session = restored;
      } else {
        session = await this.options.transport.initUpload({ fileName: file.name, size: file.size });
        if (this.cancelled) {
          // Cancelled while the server was minting the id: delete the empty
          // session instead of leaving it for the TTL sweep.
          try {
            await this.options.transport.cancelUpload(session.id);
          } catch {
            // Already gone.
          }
          return;
        }
        this.update({
          session,
          total: session.size,
          uploaded: session.receivedBytes,
        });
        this.store();
      }

      const open = session;
      const received = new Set(open.receivedParts);
      let current = open;
      for (let part = 0; part < open.totalParts; part += 1) {
        if (this.cancelled) return;
        if (received.has(part)) continue;
        const { start, end } = chunkRange(open, part);
        const response = await this.options.transport.uploadPart(
          current.id,
          part,
          file.slice(start, end),
          (loaded) => {
            if (this.cancelled) return;
            this.update({ uploaded: Math.min(current.size, start + loaded) });
          },
        );
        if (this.cancelled) return;
        received.add(part);
        current = {
          ...current,
          receivedBytes: response.receivedBytes,
          receivedParts: [...received].sort((a, b) => a - b),
        };
        this.update({ session: current, uploaded: response.receivedBytes });
      }

      if (this.cancelled) return;
      this.update({ phase: "verifying", uploaded: current.size });
      const result = await this.options.transport.completeUpload(current.id);
      if (this.cancelled) return;

      this.file = null;
      this.clearStored();
      this.update({
        phase: "complete",
        session: null,
        file: null,
        result,
        uploaded: result.size,
        total: result.size,
        error: null,
        awaitingFile: false,
      });
      this.options.onComplete?.(result);
    } catch (cause) {
      if (this.cancelled) return;
      // Keep the server session and the stored id: a failed chunk or a failed
      // verification can be retried from the parts the server already holds.
      const message = errorMessage(cause);
      this.update({ phase: "error", error: message });
      this.options.onError?.(message);
    }
  }
}
