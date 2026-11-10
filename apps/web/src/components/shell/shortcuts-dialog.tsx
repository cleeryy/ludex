import { Dialog } from "@/components/ui/dialog";
import { Kbd } from "@/components/ui/controls";

const GROUPS: { title: string; rows: { keys: string[]; label: string }[] }[] = [
  {
    title: "Global",
    rows: [
      { keys: ["⌘", "K"], label: "Open the command palette" },
      { keys: ["/"], label: "Focus library search" },
      { keys: ["g", "h"], label: "Go home" },
      { keys: ["g", "l"], label: "Go to the library" },
      { keys: ["g", "d"], label: "Go to downloads" },
      { keys: ["g", "s"], label: "Go to settings" },
      { keys: ["d"], label: "Open downloads" },
      { keys: ["?"], label: "Show this help" },
      { keys: ["esc"], label: "Close a dialog, palette or sheet" },
    ],
  },
  {
    title: "Library",
    rows: [
      { keys: ["f"], label: "Open the filter panel" },
      { keys: ["↑", "↓", "←", "→"], label: "Move through the shelf" },
      { keys: ["↵"], label: "Open the selected game" },
    ],
  },
  {
    title: "Big Picture",
    rows: [
      { keys: ["←", "→"], label: "Select a game" },
      { keys: ["↑", "↓"], label: "Change shelf" },
      { keys: ["↵"], label: "Play, install or download" },
      { keys: ["esc"], label: "Exit Big Picture" },
    ],
  },
  {
    title: "Gamepad",
    rows: [
      { keys: ["d-pad", "stick"], label: "Move focus" },
      { keys: ["A"], label: "Select" },
      { keys: ["B"], label: "Back" },
      { keys: ["X"], label: "Open filters" },
      { keys: ["Y"], label: "Open downloads" },
      { keys: ["LB", "RB"], label: "Switch tabs or shelves" },
    ],
  },
];

export function ShortcutsDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title="Keyboard shortcuts"
      description="Everything the library can do without a mouse."
      size="md"
    >
      <div className="grid gap-7 sm:grid-cols-2">
        {GROUPS.map((group) => (
          <section key={group.title} aria-label={group.title}>
            <h3 className="mb-2 font-mono text-[10px] uppercase tracking-[0.12em] text-ink-3">{group.title}</h3>
            <dl className="border-t border-line">
              {group.rows.map((row) => (
                <div key={`${group.title}-${row.label}`} className="flex items-center justify-between gap-4 border-b border-line py-2">
                  <dt className="text-[13px] text-ink-2">{row.label}</dt>
                  <dd className="flex shrink-0 items-center gap-1">
                    {row.keys.map((key) => (
                      <Kbd key={key}>{key}</Kbd>
                    ))}
                  </dd>
                </div>
              ))}
            </dl>
          </section>
        ))}
      </div>
    </Dialog>
  );
}
