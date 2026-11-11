import { Link, useLocation } from "react-router-dom";
import { ChevronRight, Keyboard, LogOut, Moon, MoreHorizontal, Sun } from "lucide-react";
import { cn } from "@/lib/cn";
import { useGenres } from "@/lib/hooks";
import { openShortcutsHelp } from "@/lib/ui-events";
import { t } from "@/lib/i18n";
import { isNavActive, mobileBar, navGroups, type NavContext, type NavDestination } from "@/lib/navigation";
import { useAuth } from "@/state/auth";
import { otherModeTheme, useSettings } from "@/state/settings";
import { Dialog } from "@/components/ui/dialog";
import { NavIcon } from "@/components/shell/nav-icons";
import { navGroupLabel, navLabel } from "@/components/shell/nav-copy";

/**
 * Phone bottom bar: the four destinations people actually use on a phone plus
 * More. Settings sits here instead of the account sheet (#174): it is a place,
 * not an action. Downloads falls back to Community when installs are offline.
 */
export function MobileBar({ context, onMore }: { context: NavContext; onMore: () => void }) {
  const { pathname } = useLocation();
  const items = mobileBar(context);

  return (
    <nav
      aria-label={t("Primary")}
      className="flex h-[calc(56px+env(safe-area-inset-bottom))] shrink-0 items-stretch border-t border-line bg-bg pb-[env(safe-area-inset-bottom)] lg:hidden"
    >
      {items.map((destination) => {
        const active = isNavActive(destination, pathname);
        return (
          <Link
            key={destination.id}
            to={destination.to}
            aria-current={active ? "page" : undefined}
            className={cn(
              "relative flex min-h-11 flex-1 flex-col items-center justify-center gap-1 text-[10px] transition-colors",
              active ? "text-ink" : "text-ink-3",
            )}
          >
            {active && <span className="absolute inset-x-5 top-0 h-[2px] bg-signal" aria-hidden="true" />}
            <NavIcon name={destination.icon} className="size-4" />
            {navLabel(destination)}
          </Link>
        );
      })}
      <button
        type="button"
        onClick={onMore}
        className="flex min-h-11 flex-1 flex-col items-center justify-center gap-1 text-[10px] text-ink-3 transition-colors hover:text-ink-2"
      >
        <MoreHorizontal className="size-4" aria-hidden="true" />
        {t("More")}
      </button>
    </nav>
  );
}

function SheetLink({ destination, onNavigate }: { destination: NavDestination; onNavigate: () => void }) {
  return (
    <Link
      to={destination.to}
      onClick={onNavigate}
      className="flex h-11 w-full items-center gap-3 border-b border-line text-[14px] text-ink-2 last:border-0 hover:text-ink"
    >
      <span className="grid size-4 place-items-center text-ink-3">
        <NavIcon name={destination.icon} className="size-3.5" />
      </span>
      {navLabel(destination)}
      <ChevronRight className="rtl:-scale-x-100 ms-auto size-3.5 text-ink-3" aria-hidden="true" />
    </Link>
  );
}

function SheetGroup({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="pt-4 first:pt-0">
      <p className="pb-1 text-[11.5px] text-ink-3">{label}</p>
      {children}
    </div>
  );
}

/**
 * The More sheet keeps parity with the rail: same table, same labels, same
 * visibility. Destinations already in the bottom bar are skipped; everything
 * else, admin included, is one tap away.
 */
export function MoreSheet({
  open,
  onOpenChange,
  context,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  context: NavContext;
}) {
  const genres = useGenres();
  const { resolved, setTheme } = useSettings();
  const { signOut, user } = useAuth();
  const inBar = new Set(mobileBar(context).map((destination) => destination.id));
  const groups = navGroups(context)
    .map((entry) => ({
      group: entry.group,
      items: entry.items.filter((destination) => !inBar.has(destination.id)),
    }))
    .filter((entry) => entry.items.length > 0);
  const close = () => onOpenChange(false);

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title={t("Menu")}
      description={user ? t("Signed in as {name}", { name: user.username }) : undefined}
      size="sm"
    >
      <div className="max-h-[60vh] overflow-y-auto">
        {groups.map((entry) => (
          <SheetGroup key={entry.group.id} label={navGroupLabel(entry.group.id)}>
            {entry.items.map((destination) => (
              <SheetLink key={destination.id} destination={destination} onNavigate={close} />
            ))}
          </SheetGroup>
        ))}

        {genres.length > 0 && (
          <SheetGroup label={t("Genres")}>
            <div className="flex flex-wrap gap-1.5 pb-3">
              {genres.map((genre) => (
                <Link
                  key={genre}
                  to={`/library?genre=${encodeURIComponent(genre)}`}
                  onClick={close}
                  className="flex h-11 items-center rounded-[2px] border border-line px-3 text-[12.5px] text-ink-2 transition-colors hover:border-line-2 hover:text-ink"
                >
                  {genre}
                </Link>
              ))}
            </div>
          </SheetGroup>
        )}

        <SheetGroup label={t("This device")}>
          <button
            type="button"
            onClick={() => {
              close();
              openShortcutsHelp();
            }}
            className="flex h-11 w-full items-center gap-3 border-b border-line text-left text-[14px] text-ink-2 hover:text-ink"
          >
            <span className="grid size-4 place-items-center text-ink-3">
              <Keyboard className="size-3.5" aria-hidden="true" />
            </span>
            {t("Keyboard shortcuts")}
          </button>
          <button
            type="button"
            onClick={() => {
              close();
              setTheme(otherModeTheme(resolved));
            }}
            className="flex h-11 w-full items-center gap-3 border-b border-line text-left text-[14px] text-ink-2 hover:text-ink"
          >
            <span className="grid size-4 place-items-center text-ink-3">
              {resolved === "dark" ? <Sun className="size-3.5" aria-hidden="true" /> : <Moon className="size-3.5" aria-hidden="true" />}
            </span>
            {resolved === "dark" ? t("Switch to the light theme") : t("Switch to the dark theme")}
          </button>
          <button
            type="button"
            onClick={() => {
              close();
              void signOut();
            }}
            className="flex h-11 w-full items-center gap-3 text-left text-[14px] text-danger hover:opacity-80"
          >
            <span className="grid size-4 place-items-center">
              <LogOut className="size-3.5" aria-hidden="true" />
            </span>
            {t("Sign out")}
          </button>
        </SheetGroup>
      </div>
    </Dialog>
  );
}
