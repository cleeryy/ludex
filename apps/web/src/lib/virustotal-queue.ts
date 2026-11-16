/**
 * VirusTotal bulk-scan queue (#209/#224/#238).
 *
 * The public API allows 4 requests per minute, so the bulk action is a client
 * scheduler. #238 moved that scheduler out of the Games screen: it is a plain
 * controller with no React import, owned by `state/virustotal-queue.tsx` above
 * the router. Route changes, and even a provider remount, therefore cannot
 * cancel a run — only `cancel()` or a drained queue stops it.
 *
 * Schedule: one request per 15 s slot, the first one immediately. A slot that
 * is appended while the queue runs waits for the same 15 s budget instead of
 * firing in a burst, because the next start time is tracked as a timestamp and
 * not recomputed from the list length.
 */

import { VIRUSTOTAL_RATE_PER_MINUTE } from "@/lib/virustotal";
import { t } from "@/lib/i18n";

export interface VirusTotalQueueRun {
  /** Unique ids of the run, in start order, handled ones included. */
  ids: number[];
  /** Scans whose request resolved and whose report cache was invalidated. */
  done: number;
  /** Scans whose request rejected; the run keeps going. */
  failed: number;
  startedAt: number;
}

export interface VirusTotalQueueSnapshot {
  queue: VirusTotalQueueRun | null;
  /** Message of the last rejection; cleared when a new run starts. */
  error: string | null;
  /** Game whose request is in flight, `null` between slots. */
  activeId: number | null;
}

export interface VirusTotalQueueStart {
  /** Ids added by this call; 0 when every id was already known. */
  accepted: number;
  /** `true` when the call joined a run that was already going. */
  appended: boolean;
}

export interface VirusTotalQueueOptions {
  /** One hash lookup; the server owns the actual VirusTotal call. */
  scan: (gameId: number) => Promise<unknown>;
  /** Refresh that game's cached report after its scan lands. */
  invalidateReport: (gameId: number) => Promise<unknown> | void;
  /** Called once when a run drains (never on cancel). */
  onDrained: (result: { done: number; failed: number; startedAt: number }) => void;
  /** Refresh the admin table, the integration card and every report. */
  invalidateDrained: () => void;
  /** Budget override for tests; production keeps the public 4/min limit. */
  perMinute?: number;
  /** Clock override for tests; production reads `Date.now`. */
  now?: () => number;
}

const EMPTY: VirusTotalQueueSnapshot = Object.freeze({ queue: null, error: null, activeId: null });

/**
 * What the provider hands the queue: the API call, the cache invalidation and
 * the toast. Keeping this behind a small interface lets the drained-run wiring
 * (query keys, completion copy) be tested without React.
 */
export interface VirusTotalQueueClient {
  /** One hash lookup; the server owns the actual VirusTotal call. */
  scan: (gameId: number) => Promise<unknown>;
  /** Invalidate one query key, or a prefix when the key is short. */
  invalidate: (queryKey: readonly unknown[]) => Promise<unknown> | void;
  /** Surface the drained run where the user is, whatever the route. */
  toast: (toast: { title: string; description: string; tone: "danger" | "success" }) => void;
}

/**
 * Provider wiring (#238). The Games table, the integration card and every
 * cached report are refreshed once the run drains, and the completion toast is
 * pushed from the app level so it appears on any route.
 */
export function createVirusTotalQueue(
  client: VirusTotalQueueClient,
  options: { perMinute?: number; now?: () => number } = {},
): VirusTotalQueue {
  return new VirusTotalQueue({
    scan: (gameId) => client.scan(gameId),
    invalidateReport: (gameId) => client.invalidate(["game-virustotal", gameId]),
    invalidateDrained: () => {
      void client.invalidate(["admin-games"]);
      void client.invalidate(["admin-virustotal"]);
      // Prefix invalidation: every report, including the game detail page.
      void client.invalidate(["game-virustotal"]);
    },
    onDrained: ({ done, failed }) => {
      client.toast({
        title: t("VirusTotal scan queue finished"),
        description:
          failed > 0
            ? t("{done} scanned, {failed} failed. Rescan the failed ones when you want.", { done, failed })
            : t("{count} archives scanned. Verdicts come from the report cache.", { count: done }),
        tone: failed > 0 ? "danger" : "success",
      });
    },
    perMinute: options.perMinute,
    now: options.now,
  });
}

/**
 * Second-start policy (#238): **append**. A `start` while a run is going adds
 * the new ids to the end of the run and ignores ids that are already queued,
 * in flight or handled, so per-row "Scan" calls can never duplicate a request.
 * The schedule is not restarted, and the 4/min budget is never exceeded.
 */
export class VirusTotalQueue {
  private readonly options: VirusTotalQueueOptions;
  private readonly intervalMs: number;
  private readonly listeners = new Set<() => void>();
  private snapshot: VirusTotalQueueSnapshot = EMPTY;
  private run: VirusTotalQueueRun | null = null;
  private pending: number[] = [];
  /** Every id of the current run: queued, in flight or handled. */
  private known = new Set<number>();
  private activeId: number | null = null;
  private timer: ReturnType<typeof setTimeout> | null = null;
  /** Earliest time the next request may start. */
  private nextAt = 0;
  /** Message of the last rejection of the current run. */
  private errorMessage: string | null = null;
  /** Bumped on a new run and on cancel, so late responses are ignored. */
  private token = 0;

  constructor(options: VirusTotalQueueOptions) {
    this.options = options;
    this.intervalMs = 60_000 / Math.max(1, options.perMinute ?? VIRUSTOTAL_RATE_PER_MINUTE);
  }

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  getSnapshot = (): VirusTotalQueueSnapshot => this.snapshot;

  /** Requests still queued, the in-flight one included. */
  get remaining(): number {
    if (!this.run) return 0;
    return Math.max(0, this.run.ids.length - this.run.done - this.run.failed);
  }

  start(ids: number[]): VirusTotalQueueStart {
    const appended = this.run !== null;
    const fresh: number[] = [];
    for (const id of ids) {
      if (!Number.isFinite(id) || this.known.has(id)) continue;
      this.known.add(id);
      fresh.push(id);
    }
    if (fresh.length === 0) return { accepted: 0, appended };
    const now = this.now();
    if (!this.run) {
      this.run = { ids: [], done: 0, failed: 0, startedAt: now };
      this.nextAt = now;
      this.errorMessage = null;
      this.token += 1;
    }
    this.run.ids.push(...fresh);
    this.pending.push(...fresh);
    this.emit();
    this.pump();
    return { accepted: fresh.length, appended };
  }

  /** Stop the run: no timer survives, no further request fires. */
  cancel(): void {
    if (!this.run && this.timer === null) return;
    this.token += 1;
    this.clearTimer();
    this.run = null;
    this.pending = [];
    this.known.clear();
    this.activeId = null;
    this.errorMessage = null;
    this.emit();
  }

  private now(): number {
    return this.options.now ? this.options.now() : Date.now();
  }

  private clearTimer(): void {
    if (this.timer === null) return;
    clearTimeout(this.timer);
    this.timer = null;
  }

  private pump(): void {
    if (!this.run || this.timer !== null || this.activeId !== null) return;
    if (this.pending.length === 0) {
      this.finish();
      return;
    }
    const delay = Math.max(0, this.nextAt - this.now());
    const token = this.token;
    this.timer = setTimeout(() => {
      this.timer = null;
      void this.fire(token);
    }, delay);
  }

  private async fire(token: number): Promise<void> {
    const run = this.run;
    if (!run || token !== this.token) return;
    const gameId = this.pending.shift();
    if (gameId === undefined) return;
    this.activeId = gameId;
    // The slot is consumed when the request starts, like the old schedule.
    this.nextAt = this.now() + this.intervalMs;
    this.emit();
    try {
      await this.options.scan(gameId);
      if (token !== this.token) return;
      await this.options.invalidateReport(gameId);
      if (token !== this.token) return;
      run.done += 1;
    } catch (cause) {
      if (token !== this.token) return;
      run.failed += 1;
      this.errorMessage = cause instanceof Error ? cause.message : String(cause);
    } finally {
      if (token === this.token) {
        this.activeId = null;
        this.emit();
        this.pump();
      }
    }
  }

  /** Last scan settled: refresh the caches, toast once, then forget the run. */
  private finish(): void {
    const run = this.run;
    if (!run) return;
    this.clearTimer();
    this.run = null;
    this.pending = [];
    this.known.clear();
    this.activeId = null;
    this.emit();
    this.options.invalidateDrained();
    this.options.onDrained({ done: run.done, failed: run.failed, startedAt: run.startedAt });
  }

  private emit(): void {
    const run = this.run;
    this.snapshot = {
      queue: run
        ? { ids: [...run.ids], done: run.done, failed: run.failed, startedAt: run.startedAt }
        : null,
      error: this.errorMessage,
      activeId: this.activeId,
    };
    for (const listener of this.listeners) listener();
  }
}
