import type { ProgressState } from "@/lib/types";
import { stateLabel } from "@/lib/format";
import { cn } from "@/lib/cn";

export const STATE_TONE: Record<ProgressState, { dot: string; text: string; bar: string }> = {
  unplayed: { dot: "bg-line-2", text: "text-ink-3", bar: "bg-line-2" },
  playing: { dot: "bg-signal", text: "text-signal-text", bar: "bg-signal" },
  completed: { dot: "bg-ink-2", text: "text-ink-2", bar: "bg-ink-2" },
  abandoned: { dot: "bg-danger", text: "text-danger", bar: "bg-danger" },
  onHold: { dot: "bg-ink-3", text: "text-ink-3", bar: "bg-ink-3" },
  planToPlay: { dot: "border border-line-2 bg-transparent", text: "text-ink-3", bar: "bg-line-2" },
};

export function StateMark({ state, className }: { state: ProgressState; className?: string }) {
  const tone = STATE_TONE[state];
  return (
    <span className={cn("inline-flex items-center gap-1.5 text-[11.5px]", tone.text, className)}>
      <span className={cn("size-1.5 shrink-0 rounded-full", tone.dot)} aria-hidden="true" />
      {stateLabel(state)}
    </span>
  );
}
