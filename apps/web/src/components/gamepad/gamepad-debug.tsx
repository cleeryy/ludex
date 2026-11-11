import type { GamepadDebugInfo } from "@/lib/gamepad";
import { cn } from "@/lib/cn";

/**
 * Diagnostic overlay for real controllers (#137). Enabled only by
 * `?gamepadDebug=1` (also works after the hash), so it never ships as visible
 * noise. Developer-facing labels stay in plain English on purpose: it is not
 * part of the translated interface.
 */
export function GamepadDebugPanel({ info }: { info: GamepadDebugInfo | null }) {
  return (
    <aside
      aria-label="Gamepad diagnostics"
      className={cn(
        "pointer-events-none fixed bottom-3 left-3 z-[120] w-[310px] rounded-[2px] border border-line-2 bg-bg-deep/95 p-3",
        "font-mono text-[10px] leading-relaxed text-ink-2 shadow-[0_18px_40px_-18px_rgba(0,0,0,0.9)]",
      )}
    >
      <p className="mb-1.5 flex items-center justify-between text-signal-text">
        <span>gamepad debug</span>
        <span className="text-ink-3">?gamepadDebug=1</span>
      </p>
      {info ? (
        <div className="space-y-1">
          <Row label="pad" value={info.id ?? "—"} />
          <Row label="index" value={info.index === null ? "—" : String(info.index)} />
          <Row label="mapping" value={info.mapping || "(none)"} />
          <Row label="profile" value={info.profile} />
          <Row
            label="axes"
            value={info.axes.length ? info.axes.map((value) => value.toFixed(2)).join(" ") : "—"}
          />
          <Row
            label="buttons"
            value={
              info.buttons.length
                ? info.buttons.map((button) => `${button.index}:${button.value}`).join(" ")
                : "—"
            }
          />
          <Row
            label="action"
            value={info.action ? `${info.action}` : "(none yet)"}
            tone={info.action ? "signal" : undefined}
          />
          <p className="pt-1 text-ink-3">
            {info.connected ? "streaming" : "no pad — press a button on the controller"}
          </p>
        </div>
      ) : (
        <p className="text-ink-3">Waiting for a controller — press a button.</p>
      )}
    </aside>
  );
}

function Row({
  label,
  value,
  tone,
}: {
  label: string;
  value: string;
  tone?: "signal";
}) {
  return (
    <p className="flex gap-2">
      <span className="w-[52px] shrink-0 uppercase tracking-[0.08em] text-ink-3">{label}</span>
      <span className={cn("min-w-0 break-all", tone === "signal" ? "text-signal-text" : "text-ink")}>
        {value}
      </span>
    </p>
  );
}
