import { useEffect, useRef, type ReactNode } from "react";
import { FolderOpen } from "lucide-react";
import { cn } from "@/lib/cn";
import { t } from "@/lib/i18n";
import { SectionHeader } from "@/components/page";
import { Button, Switch } from "@/components/ui/controls";
import type { DesktopSettingsController } from "@/components/settings/use-desktop-settings";

/**
 * One settings category. Renders the section heading (except for a category
 * whose first panel already owns one), names the region with it, and moves
 * focus there when the user picks the category or follows a deep link (#174).
 */
export function SettingsSection({
  id,
  title,
  description,
  heading = "section",
  focusToken = 0,
  children,
}: {
  id: string;
  title: string;
  description?: string;
  /** `panel`: the first child renders the heading (Language, Compatibility). */
  heading?: "section" | "panel";
  /** Grew since mount? Then the heading takes focus. 0 keeps the page as is. */
  focusToken?: number;
  children: ReactNode;
}) {
  const ref = useRef<HTMLElement>(null);
  const mounted = useRef(false);
  const headingId = `settings-${id}-heading`;

  useEffect(() => {
    const section = ref.current;
    if (!section) return;
    section.closest<HTMLElement>("[data-scroll-root]")?.scrollTo({ top: 0, behavior: "auto" });
    // The first render keeps the page as the browser left it (no focus theft);
    // every later category change — click, deep link, history, palette — moves
    // focus to the heading so a screen reader starts at the new section.
    const firstRender = !mounted.current;
    mounted.current = true;
    if (firstRender && focusToken <= 0) return;
    const heading = section.querySelector<HTMLElement>("h2, h3");
    if (!heading) return;
    // Programmatic focus needs an explicit tabindex; -1 keeps it out of Tab.
    // The heading is not an interactive target, so the focus ring would read as
    // a selection box around the title; keep the movement, drop the paint.
    heading.tabIndex = -1;
    heading.style.outline = "none";
    heading.focus({ preventScroll: true });
  }, [focusToken, id]);

  return (
    <section
      ref={ref}
      aria-labelledby={heading === "section" ? headingId : undefined}
      aria-label={heading === "panel" ? title : undefined}
      className="min-w-0"
    >
      {heading === "section" && <SectionHeader id={headingId} title={title} description={description} />}
      {children}
    </section>
  );
}

/** A named block inside a category: `h3`, 13px medium, one hairline. */
export function SettingsGroup({
  title,
  description,
  actions,
  className,
  children,
}: {
  title: string;
  description?: string;
  actions?: ReactNode;
  className?: string;
  children: ReactNode;
}) {
  return (
    <div className={cn("mt-8 border-t border-line pt-5 first:mt-0 first:border-0 first:pt-0", className)}>
      <div className="flex flex-wrap items-baseline justify-between gap-3 pb-3">
        <div>
          <h3 className="text-[13px] font-medium text-ink">{title}</h3>
          {description && <p className="mt-1 max-w-[62ch] text-[11.5px] leading-relaxed text-ink-3">{description}</p>}
        </div>
        {actions}
      </div>
      {children}
    </div>
  );
}

/** A title + explanation row with its switch at the trailing edge. */
export function SettingsSwitchRow({
  title,
  description,
  checked,
  onCheckedChange,
}: {
  title: string;
  description: string;
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
}) {
  return (
    <div className="flex items-start justify-between gap-6 border-b border-line py-4">
      <div className="min-w-0">
        <p className="text-[13px] font-medium text-ink">{title}</p>
        <p className="mt-1 max-w-[54ch] text-[11.5px] leading-relaxed text-ink-3">{description}</p>
      </div>
      <Switch checked={checked} onCheckedChange={onCheckedChange} label={title} />
    </div>
  );
}

/**
 * The one commit for the desktop settings file. It lives inside the categories
 * that edit that file (Storage, Downloads, Saves, Integrations, Updates,
 * Compatibility), next to the controls it saves, instead of floating at the
 * bottom of a three-hundred-line page.
 */
export function DesktopSaveBar({
  desktop,
  openFolder = false,
}: {
  desktop: DesktopSettingsController;
  openFolder?: boolean;
}) {
  if (!desktop.draft) return null;
  return (
    <div className="mt-8 flex flex-wrap items-center gap-3 border-t border-line pt-4">
      <Button variant="primary" onClick={() => void desktop.save()} loading={desktop.saving} disabled={!desktop.dirty}>
        {t("Save desktop settings")}
      </Button>
      {desktop.dirty && (
        <span role="status" className="text-[11.5px] text-ink-3">
          {t("Unsaved changes")}
        </span>
      )}
      {openFolder && desktop.draft.installDir && (
        <Button
          variant="ghost"
          size="sm"
          className="ms-auto text-ink-3"
          icon={<FolderOpen className="size-3.5" />}
          onClick={desktop.openInstallFolder}
        >
          {t("Open install folder")}
        </Button>
      )}
    </div>
  );
}

/** Shown where a category only exists in the desktop app. */
export function DesktopOnlyNote({ message }: { message?: string }) {
  return (
    <p className="max-w-[70ch] text-[11.5px] leading-relaxed text-ink-3">
      {message ?? t("Install folders, stream installs and archive passwords are available in the Ludex desktop app.")}
    </p>
  );
}

export function SettingsLoading({ label }: { label: string }) {
  return <p className="py-6 text-[13px] text-ink-3">{label}</p>;
}
