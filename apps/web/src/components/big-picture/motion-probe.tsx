import { useEffect, useRef, useState } from "react";
import { MOTION, type TransitionHub } from "./motion";

/* --------------------------------------------------------------------------
   Frame-budget probe (#220).

   Dev-only, behind `?motionDebug=1` (also read from the hash query). It watches
   the real rAF cadence while the player drives Big Picture, then reports the
   last transition, the worst frame and how many frames missed the 60 fps
   budget. English labels on purpose: this is a developer surface, like the
   gamepad panel, and it is never part of the translated interface.
   -------------------------------------------------------------------------- */

interface ProbeStats {
  fps: number;
  worst: number;
  over16: number;
  over33: number;
  kind: string;
  budget: number;
  instant: boolean;
  cuts: number;
  frames: number;
}

const EMPTY: ProbeStats = {
  fps: 0,
  worst: 0,
  over16: 0,
  over33: 0,
  kind: "—",
  budget: 0,
  instant: false,
  cuts: 0,
  frames: 0,
};

export function MotionProbe({ transitions }: { transitions: TransitionHub }) {
  const [stats, setStats] = useState<ProbeStats>(EMPTY);
  const frames = useRef<number[]>([]);

  useEffect(() => {
    let raf = 0;
    let last = performance.now();
    let published = last;

    const tick = (now: number) => {
      const delta = now - last;
      last = now;
      frames.current.push(delta);
      if (frames.current.length > 240) frames.current.shift();

      if (now - published >= 500) {
        published = now;
        const sample = frames.current;
        const active = transitions.last();
        const spec = active ? MOTION[active.kind] : null;
        setStats({
          fps: sample.length ? Math.round((1000 * sample.length) / sample.reduce((sum, value) => sum + value, 0)) : 0,
          worst: Math.round(Math.max(0, ...sample)),
          over16: sample.filter((value) => value > 17).length,
          over33: sample.filter((value) => value > 33).length,
          kind: active?.kind ?? "—",
          budget: spec ? Math.round(spec.duration * 1000) : 0,
          instant: active?.instant ?? false,
          cuts: transitions.interruptions(),
          frames: sample.length,
        });
      }
      raf = window.requestAnimationFrame(tick);
    };

    raf = window.requestAnimationFrame(tick);
    return () => window.cancelAnimationFrame(raf);
  }, [transitions]);

  const tone = stats.over33 > 0 ? "text-danger" : stats.over16 > 2 ? "text-warn" : "text-ok";

  return (
    <aside
      aria-label="Motion diagnostics"
      className="pointer-events-none fixed bottom-3 end-3 z-[120] w-[286px] rounded-[2px] border border-line-2 bg-bg-deep/95 p-3 font-mono text-[10px] leading-relaxed text-ink-2 shadow-debug"
    >
      <p className="mb-1.5 flex items-center justify-between text-signal-text">
        <span>motion debug</span>
        <span className="text-ink-3">?motionDebug=1</span>
      </p>
      <div className="space-y-1">
        <Row label="fps" value={String(stats.fps)} tone={tone} />
        <Row label="worst" value={`${stats.worst} ms`} tone={stats.worst > 33 ? "text-danger" : undefined} />
        <Row label=">17" value={`${stats.over16} / ${stats.frames}`} />
        <Row label=">33.3" value={`${stats.over33} / ${stats.frames}`} />
        <Row label="last" value={stats.kind} tone={stats.kind === "—" ? undefined : "text-signal-text"} />
        <Row label="budget" value={stats.budget ? `${stats.budget} ms` : "—"} />
        <Row label="mode" value={stats.instant ? "instant (held)" : "tween"} />
        <Row label="cuts" value={String(stats.cuts)} />
      </div>
      <p className="pt-1 text-ink-3">transform/opacity only · 60 fps budget</p>
    </aside>
  );
}

function Row({ label, value, tone }: { label: string; value: string; tone?: string }) {
  return (
    <p className="flex items-baseline justify-between gap-3">
      <span className="text-ink-3">{label}</span>
      <span className={tone ? `num ${tone}` : "num"}>{value}</span>
    </p>
  );
}
