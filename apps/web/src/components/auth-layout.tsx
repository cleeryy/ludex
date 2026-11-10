import type { ReactNode } from "react";
import { GameCover } from "@/components/art/cover";
import { Wordmark } from "@/components/brand";
import { FloatingWindowControls } from "@/components/shell/window-controls";
import { demoCover } from "@/lib/art";
import { t } from "@/lib/i18n";
import { useCustomTitleBar } from "@/lib/window-chrome";

const FRIEZE = ["outer-wilds", "blue-prince", "little-nightmares-iii"];

export function AuthLayout({
  children,
  statement,
  note,
  footer,
}: {
  children: ReactNode;
  statement: string;
  note: string;
  footer?: ReactNode;
}) {
  // Login and setup live outside the AppShell, so they carry their own window
  // chrome: a drag surface on the wordmark rows and the controls in the corner.
  const customChrome = useCustomTitleBar();

  return (
    <div className="grid min-h-dvh grid-rows-[auto_1fr] bg-bg lg:grid-cols-[1.12fr_1fr] lg:grid-rows-1">
      <aside className="relative hidden overflow-hidden border-r border-line bg-bg-deep lg:flex lg:flex-col">
        <div
          data-tauri-drag-region={customChrome ? "deep" : undefined}
          className="flex items-center justify-between px-10 pt-8"
        >
          <Wordmark height={20} />
          <span className="font-mono text-[10px] uppercase tracking-[0.16em] text-ink-3">{t("Self-hosted")}</span>
        </div>

        <div className="flex flex-1 items-center justify-center px-10">
          <div className="grid w-full max-w-[520px] grid-cols-3 items-end gap-3" aria-hidden="true">
            {FRIEZE.map((slug, index) => (
              <div
                key={slug}
                className={
                  index === 0
                    ? "-rotate-[3deg] translate-y-4 opacity-90"
                    : index === 1
                      ? "z-10 -translate-y-2"
                      : "rotate-[2.5deg] translate-y-6 opacity-90"
                }
              >
                <GameCover title={slug} coverUrl={demoCover(slug)} showType={false} />
              </div>
            ))}
          </div>
        </div>

        <div className="px-10 pb-10">
          <p className="max-w-[30ch] font-display text-[27px] font-semibold leading-[1.08] tracking-[-0.025em]">{statement}</p>
          <p className="mt-3 max-w-[46ch] text-[13px] leading-relaxed text-ink-3">{note}</p>
        </div>
      </aside>

      <main className="flex flex-col px-[var(--page-x)] py-8">
        {/* Top strip: the wordmark on small screens, an empty drag handle for
            the undecorated window on large ones. */}
        <div data-tauri-drag-region={customChrome ? "deep" : undefined} className="flex min-h-9 items-center">
          <span className="lg:hidden">
            <Wordmark height={17} />
          </span>
        </div>
        <div className="flex flex-1 items-center">
          <div className="w-full max-w-[392px] py-10 lg:mx-auto">{children}</div>
        </div>
        <footer className="flex items-center justify-between gap-4 text-[11px] text-ink-3">
          <span className="num">v0.1.0</span>
          {footer ?? (
            <a
              href="https://github.com/cleeryy/ludex/blob/main/LICENSE"
              target="_blank"
              rel="noopener noreferrer"
              className="hover:text-ink"
            >
              AGPL-3.0
            </a>
          )}
        </footer>
      </main>
      <FloatingWindowControls />
    </div>
  );
}
