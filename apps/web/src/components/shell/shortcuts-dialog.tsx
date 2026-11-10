import { Dialog } from "@/components/ui/dialog";
import { Kbd } from "@/components/ui/controls";
import { t } from "@/lib/i18n";

function shortcutGroups(): { title: string; rows: { keys: string[]; label: string }[] }[] {
  return [
  {
    title: t("Global"),
    rows: [
      { keys: ["⌘", "K"], label: t("Open the command palette") },
      { keys: ["/"], label: t("Focus library search") },
      { keys: ["g", "h"], label: t("Go home") },
      { keys: ["g", "l"], label: t("Go to the library") },
      { keys: ["g", "d"], label: t("Go to downloads") },
      { keys: ["g", "s"], label: t("Go to settings") },
      { keys: ["d"], label: t("Open downloads") },
      { keys: ["?"], label: t("Show this help") },
      { keys: ["esc"], label: t("Close a dialog, palette or sheet") },
    ],
  },
  {
    title: t("Library"),
    rows: [
      { keys: ["f"], label: t("Open the filter panel") },
      { keys: ["↑", "↓", "←", "→"], label: t("Move through the shelf") },
      { keys: ["↵"], label: t("Open the selected game") },
    ],
  },
  {
    title: t("Big Picture"),
    rows: [
      { keys: ["←", "→"], label: t("Select a game") },
      { keys: ["↑", "↓"], label: t("Change shelf") },
      { keys: ["↵"], label: t("Play, install or download") },
      { keys: ["esc"], label: t("Exit Big Picture") },
    ],
  },
  {
    title: t("Gamepad"),
    rows: [
      { keys: ["d-pad", "stick"], label: t("Move focus") },
      { keys: ["A"], label: t("Select") },
      { keys: ["B"], label: t("Back") },
      { keys: ["X"], label: t("Open filters") },
      { keys: ["Y"], label: t("Open downloads") },
      { keys: ["LB", "RB"], label: t("Switch tabs or shelves") },
    ],
  },
  ];
}

export function ShortcutsDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title={t("Keyboard shortcuts")}
      description={t("Everything the library can do without a mouse.")}
      size="md"
    >
      <div className="grid gap-7 sm:grid-cols-2">
        {shortcutGroups().map((group) => (
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
