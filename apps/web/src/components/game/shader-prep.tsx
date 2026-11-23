import { useEffect, useMemo, useState } from "react";
import { Link } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Eraser, Flame, Square } from "lucide-react";
import { errorMessage } from "@/lib/http";
import { formatBytes, formatDuration, formatRelative } from "@/lib/format";
import { useAllGames } from "@/lib/hooks";
import { isNative, native } from "@/lib/native";
import { qk } from "@/lib/query-keys";
import type { ShaderPrepState, ShaderPrepView } from "@/lib/types";
import { isShaderPrepActive, shaderPrepHasCache, shaderPrepTone } from "@/lib/shader-prep";
import { useToast } from "@/state/toasts";
import { Badge, Button, Input, Progress, Switch } from "@/components/ui/controls";
import { ConfirmDialog } from "@/components/ui/dialog";
import { SectionHeader } from "@/components/page";
import { t } from "@/lib/i18n";

/** Short label of a preparation state. */
export function shaderPrepStateLabel(state: ShaderPrepState): string {
  switch (state) {
    case "queued":
      return t("Queued");
    case "running":
      return t("Preparing…");
    case "done":
      return t("Prepared");
    case "failed":
      return t("Failed");
    case "cancelled":
      return t("Cancelled");
    default:
      return t("Not prepared");
  }
}

function stateTone(state: ShaderPrepState): "default" | "signal" | "ok" | "danger" | "outline" {
  return shaderPrepTone(state);
}

function isActive(state: ShaderPrepState): boolean {
  return isShaderPrepActive(state);
}

/**
 * Per-game shader preparation (#293).
 *
 * Runs the installed game once in the background, with every shader cache
 * pointed at a folder Ludex owns, and stops it when the caches stop growing.
 * The copy is deliberately honest: no reliable headless precompile exists, so
 * the panel says a window may appear and that the run is bounded.
 */
export function ShaderPrepPanel({ gameId, title }: { gameId: number; title: string }) {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const [budget, setBudget] = useState("");
  const [busy, setBusy] = useState(false);
  const [confirmClear, setConfirmClear] = useState(false);
  const [tick, setTick] = useState<{ cacheBytes: number; elapsedMs: number } | null>(null);

  const stats = useQuery({
    queryKey: qk.shaderPrep(gameId),
    queryFn: () => native.shaderPrepStats(gameId),
    enabled: isNative(),
    staleTime: 5_000,
  });

  useEffect(() => {
    setBudget(stats.data ? String(stats.data.budgetSecs) : "");
  }, [stats.data]);

  useEffect(() => {
    if (!isNative()) return;
    let disposed = false;
    const offs: (() => void)[] = [];
    const add = (pending: Promise<() => void>) => {
      void pending.then((off) => {
        if (disposed) off();
        else offs.push(off);
      });
    };
    add(
      native.onShaderPrepProgress((event) => {
        if (event.gameId !== gameId) return;
        // One sample per second while the run is live: the panel shows the
        // cache size and the elapsed budget without refetching on every tick.
        setTick({ cacheBytes: event.cacheBytes, elapsedMs: event.elapsedMs });
      }),
    );
    add(
      native.onShaderPrepChanged(() => {
        void queryClient.invalidateQueries({ queryKey: qk.shaderPrep(gameId) });
        void queryClient.invalidateQueries({ queryKey: qk.shaderPrepAll });
      }),
    );
    return () => {
      disposed = true;
      for (const off of offs) off();
    };
  }, [gameId, queryClient]);

  const view: ShaderPrepView | undefined = stats.data;
  const state = view?.state ?? "never";
  const running = state === "running";
  const active = isActive(state);

  const refresh = async () => {
    await queryClient.invalidateQueries({ queryKey: qk.shaderPrep(gameId) });
    await queryClient.invalidateQueries({ queryKey: qk.shaderPrepAll });
  };

  const toggleEnabled = async (enabled: boolean) => {
    setBusy(true);
    try {
      await native.setShaderPrepEnabled(gameId, enabled);
      await refresh();
      push({
        title: enabled ? t("Shader preparation enabled") : t("Shader preparation disabled"),
        description: enabled
          ? t("{title} prepares its shaders after each install.", { title })
          : t("{title} never prepares shaders automatically.", { title }),
        tone: "success",
      });
    } catch (cause) {
      push({
        title: t("Could not change the shader preparation"),
        description: errorMessage(cause),
        tone: "danger",
      });
    } finally {
      setBusy(false);
    }
  };

  const prepare = async () => {
    setBusy(true);
    try {
      await native.prepareGameShaders(gameId);
      await refresh();
      push({
        title: t("Shader preparation queued"),
        description: t(
          "{title} runs once in the background, then Ludex stops it once the caches stop growing. The first launch is never blocked.",
          { title },
        ),
        tone: "success",
      });
    } catch (cause) {
      push({
        title: t("Could not prepare the shaders"),
        description: errorMessage(cause),
        tone: "danger",
      });
    } finally {
      setBusy(false);
    }
  };

  const cancel = async () => {
    setBusy(true);
    try {
      await native.cancelShaderPrep(gameId);
      await refresh();
      push({
        title: t("Shader preparation cancelled"),
        description: t("The partial cache is kept and the game launches normally."),
      });
    } catch (cause) {
      push({
        title: t("Could not cancel the shader preparation"),
        description: errorMessage(cause),
        tone: "danger",
      });
    } finally {
      setBusy(false);
    }
  };

  const clear = async () => {
    setConfirmClear(false);
    setBusy(true);
    try {
      await native.clearShaderCache(gameId);
      await refresh();
      push({
        title: t("Shader cache cleared"),
        description: t("The next launch compiles its pipelines again and may stutter."),
        tone: "success",
      });
    } catch (cause) {
      push({
        title: t("Could not clear the shader cache"),
        description: errorMessage(cause),
        tone: "danger",
      });
    } finally {
      setBusy(false);
    }
  };

  const saveBudget = async () => {
    const seconds = Number(budget);
    if (!Number.isFinite(seconds) || seconds <= 0) return;
    setBusy(true);
    try {
      const saved = await native.setShaderPrepBudget(Math.round(seconds));
      setBudget(String(saved));
      await refresh();
      push({
        title: t("Time budget saved"),
        description: t("One warm start stops after {duration}.", {
          duration: formatDuration(saved),
        }),
        tone: "success",
      });
    } catch (cause) {
      push({
        title: t("Could not save the time budget"),
        description: errorMessage(cause),
        tone: "danger",
      });
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="min-w-0">
      <SectionHeader
        level={3}
        title={t("Shader preparation")}
        description={t("Fill the graphics caches before the first launch")}
      />
      <div className="border-t border-line pt-4">
        {!view ? (
          <p className="text-label text-ink-3">{t("Reading the cache…")}</p>
        ) : !view.supported ? (
          <p className="max-w-[70ch] text-label leading-relaxed text-ink-2">
            {t(
              "Shader preparation is only available on Linux, where DXVK, VKD3D-Proton and the Mesa drivers keep their caches in files Ludex can fill.",
            )}
          </p>
        ) : (
          <>
            <div className="flex items-start justify-between gap-4">
              <div className="min-w-0">
                <p className="text-label text-ink-2">{t("Prepare after install")}</p>
                <p className="mt-0.5 max-w-[70ch] text-meta leading-relaxed text-ink-3">
                  {t(
                    "Runs {title} once in the background after an install, then stops it once the caches stop growing. It never blocks the first launch, and a failure leaves the game untouched.",
                    { title },
                  )}
                </p>
              </div>
              <Switch
                checked={view.enabled}
                disabled={busy || running}
                onCheckedChange={(enabled) => void toggleEnabled(enabled)}
                label={t("Prepare after install")}
              />
            </div>

            <div className="mt-4 grid gap-4 sm:grid-cols-2">
              <div>
                <p className="text-meta uppercase tracking-[0.06em] text-ink-3">{t("Cache size")}</p>
                <p className="num mt-1 text-lead text-ink">{formatBytes(view.cacheBytes)}</p>
              </div>
              <div>
                <p className="text-meta uppercase tracking-[0.06em] text-ink-3">
                  {t("Last prepared")}
                </p>
                <p className="mt-1 text-body text-ink">
                  {view.preparedAt ? formatRelative(view.preparedAt) : t("Never prepared")}
                </p>
              </div>
            </div>

            {active && (
              <div className="mt-4">
                <Progress
                  value={
                    tick && view.budgetSecs > 0
                      ? Math.min(99, (tick.elapsedMs / (view.budgetSecs * 1000)) * 100)
                      : null
                  }
                  label={t("Preparing shaders…")}
                />
                <p className="mt-1.5 font-mono text-micro text-ink-3">
                  {formatBytes(tick?.cacheBytes ?? view.cacheBytes)} ·{" "}
                  {formatDuration(Math.round((tick?.elapsedMs ?? 0) / 1000))} /{" "}
                  {formatDuration(view.budgetSecs)}
                </p>
              </div>
            )}

            {view.message && (
              <p className="mt-3 border-l-2 border-line-2 pl-3 text-meta leading-relaxed text-ink-2">
                {view.message}
              </p>
            )}

            <div className="mt-4 flex flex-wrap items-center gap-2">
              {active ? (
                <Button variant="ghost" loading={busy} onClick={() => void cancel()}>
                  <Square className="size-3.5" aria-hidden="true" />
                  {running ? t("Cancel preparation") : t("Remove from queue")}
                </Button>
              ) : (
                <Button variant="default" loading={busy} onClick={() => void prepare()}>
                  <Flame className="size-3.5" aria-hidden="true" />
                  {t("Prepare now")}
                </Button>
              )}
              <Button
                variant="ghost"
                disabled={busy || active || !shaderPrepHasCache(view.cacheBytes)}
                onClick={() => setConfirmClear(true)}
              >
                <Eraser className="size-3.5" aria-hidden="true" />
                {t("Clear cache")}
              </Button>
              <Badge tone={stateTone(state)}>{shaderPrepStateLabel(state)}</Badge>
            </div>

            <div className="mt-5 flex flex-wrap items-end gap-3">
              <label className="flex flex-col gap-1.5">
                <span className="text-label text-ink-3">{t("Time budget (seconds)")}</span>
                <Input
                  type="number"
                  inputMode="numeric"
                  min={30}
                  max={900}
                  value={budget}
                  onChange={(event) => setBudget(event.target.value)}
                  className="num h-10 w-28 font-mono text-label"
                  aria-label={t("Time budget (seconds)")}
                />
              </label>
              <Button
                variant="ghost"
                disabled={busy || budget === String(view.budgetSecs)}
                onClick={() => void saveBudget()}
              >
                {t("Save budget")}
              </Button>
            </div>

            <p className="mt-3 max-w-[70ch] text-meta leading-relaxed text-ink-3">
              {t(
                "No reliable headless precompile exists: the game may open a window while it warms up, and Ludex stops it after the caches stop growing or the budget ends. The game is always launchable, prepared or not.",
              )}
            </p>
            <p className="mt-1 font-mono text-micro text-ink-3">{view.cachePath}</p>
          </>
        )}
      </div>

      <ConfirmDialog
        open={confirmClear}
        onOpenChange={setConfirmClear}
        title={t("Clear the shader cache?")}
        description={t(
          "{title} loses its prepared caches. The next launch compiles its pipelines again and the first minutes may stutter. The game itself is not touched.",
          { title },
        )}
        confirmLabel={t("Clear cache")}
        onConfirm={() => void clear()}
      />
    </div>
  );
}

/**
 * Downloads entry for queued/running/finished preparations (#293).
 *
 * The install queue and the warm-start queue are separate: a preparation never
 * takes a download or extraction slot, but it is the same kind of background
 * work for the player, so it belongs next to the downloads.
 */
export function ShaderPrepJobs() {
  const queryClient = useQueryClient();
  const games = useAllGames();
  const [clearing, setClearing] = useState<number | null>(null);
  const desktop = isNative();

  const list = useQuery({
    queryKey: qk.shaderPrepAll,
    queryFn: () => native.listShaderPrep(),
    enabled: desktop,
    refetchInterval: (query) =>
      (query.state.data ?? []).some((entry) => isActive(entry.state)) ? 2_000 : false,
  });

  useEffect(() => {
    if (!desktop) return;
    let disposed = false;
    const offs: (() => void)[] = [];
    const add = (pending: Promise<() => void>) => {
      void pending.then((off) => {
        if (disposed) off();
        else offs.push(off);
      });
    };
    add(
      native.onShaderPrepChanged(() => {
        void queryClient.invalidateQueries({ queryKey: qk.shaderPrepAll });
      }),
    );
    return () => {
      disposed = true;
      for (const off of offs) off();
    };
  }, [desktop, queryClient]);

  const titles = useMemo(() => {
    const map = new Map<number, string>();
    for (const game of games.data?.items ?? []) map.set(game.id, game.title);
    return map;
  }, [games.data]);

  const entries = (list.data ?? []).filter((entry) => entry.state !== "never");
  if (!desktop || entries.length === 0) return null;

  const cancel = async (entry: ShaderPrepView) => {
    try {
      await native.cancelShaderPrep(entry.gameId);
      await queryClient.invalidateQueries({ queryKey: qk.shaderPrepAll });
    } catch {
      // The row disappears on the next refresh; a console-less desktop has no
      // better channel here.
    }
  };

  const clear = async (entry: ShaderPrepView) => {
    setClearing(entry.gameId);
    try {
      await native.clearShaderCache(entry.gameId);
      await queryClient.invalidateQueries({ queryKey: qk.shaderPrepAll });
      await queryClient.invalidateQueries({ queryKey: qk.shaderPrep(entry.gameId) });
    } catch {
      // Same as cancel: a failed clear is retried from the game page.
    } finally {
      setClearing(null);
    }
  };

  return (
    <section className="mt-10" aria-labelledby="downloads-shaders">
      <SectionHeader
        title={t("Shader preparation")}
        count={entries.length}
        description={t("Post-install warm starts. They never block the first launch.")}
      />
      <div id="downloads-shaders" className="border-t border-line">
        {entries.map((entry) => {
          const title = titles.get(entry.gameId) ?? t("Game #{id}", { id: entry.gameId });
          return (
            <article
              key={entry.gameId}
              className="flex flex-wrap items-center justify-between gap-3 border-b border-line py-3"
            >
              <div className="min-w-0">
                <Link
                  to={`/games/${entry.gameId}`}
                  className="text-body font-medium text-ink underline-offset-4 hover:underline"
                >
                  {title}
                </Link>
                <p className="mt-0.5 font-mono text-micro text-ink-3">
                  {formatBytes(entry.cacheBytes)}
                  {entry.preparedAt ? ` · ${formatRelative(entry.preparedAt)}` : ""}
                  {entry.state === "running" ? ` · ${t("preparing…")}` : ""}
                </p>
                {entry.state === "failed" && entry.message ? (
                  <p className="mt-1 max-w-[60ch] text-meta leading-relaxed text-ink-2">
                    {entry.message}
                  </p>
                ) : null}
              </div>
              <div className="flex items-center gap-2">
                <Badge tone={stateTone(entry.state)}>{shaderPrepStateLabel(entry.state)}</Badge>
                {isActive(entry.state) ? (
                  <Button size="sm" variant="ghost" onClick={() => void cancel(entry)}>
                    {t("Cancel")}
                  </Button>
                ) : (
                  <Button
                    size="sm"
                    variant="ghost"
                    loading={clearing === entry.gameId}
                    disabled={!shaderPrepHasCache(entry.cacheBytes) || clearing === entry.gameId}
                    onClick={() => void clear(entry)}
                  >
                    {t("Clear cache")}
                  </Button>
                )}
              </div>
            </article>
          );
        })}
      </div>
    </section>
  );
}
