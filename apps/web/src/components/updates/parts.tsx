import type { ReactNode } from "react";
import { isNative, native } from "@/lib/native";
import { cn } from "@/lib/cn";
import { t } from "@/lib/i18n";
import { edgeBuildNumber, releaseNoteBlocks } from "@/lib/updates";

/**
 * Small shared pieces of the update center (#163): an external link that opens
 * in the system browser inside Tauri, and the release-notes renderer.
 */

/** Opens http(s) links outside the webview when running as the desktop app. */
export function ExternalAnchor({
  href,
  children,
  className,
}: {
  href: string;
  children: ReactNode;
  className?: string;
}) {
  const desktop = isNative();
  return (
    <a
      href={href}
      target={desktop ? undefined : "_blank"}
      rel="noreferrer"
      onClick={(event) => {
        if (!desktop) return;
        event.preventDefault();
        void native.openExternalUrl(href).catch(() => {
          /* the address is visible in the status line; nothing to recover */
        });
      }}
      className={cn(
        "inline-flex items-center gap-1 text-ink-2 underline decoration-line-2 underline-offset-2 transition-colors hover:text-ink",
        className,
      )}
    >
      {children}
    </a>
  );
}

/**
 * Release notes as safe text blocks. Nothing from the release body is
 * interpreted as HTML; headings, bullets and paragraphs are rebuilt as React
 * elements. Automatic `edge` builds ship no notes, so the build number is shown
 * instead of an empty box.
 */
export function ReleaseNotes({
  notes,
  version,
  className,
}: {
  notes: string | null | undefined;
  version: string;
  className?: string;
}) {
  const blocks = releaseNoteBlocks(notes);
  if (blocks.length === 0) {
    const build = edgeBuildNumber(version);
    return (
      <p className={cn("measure text-[13.5px] leading-relaxed text-ink-3", className)}>
        {build
          ? t(
              "This is edge build {build}. Automatic builds do not ship written notes; the release page lists the commits in it.",
              { build },
            )
          : t("This build has no written release notes. The release page lists what changed.")}
      </p>
    );
  }

  return (
    <div className={cn("flex flex-col gap-3", className)}>
      {blocks.map((block, index) => {
        if (block.kind === "heading") {
          return (
            <h3
              key={`${block.kind}-${index}`}
              className="mt-2 font-mono text-[10.5px] uppercase tracking-[0.14em] text-ink-3 first:mt-0"
            >
              {block.text}
            </h3>
          );
        }
        if (block.kind === "bullet") {
          return (
            <p key={`${block.kind}-${index}`} className="flex gap-2.5 text-[13.5px] leading-relaxed text-ink-2">
              <span aria-hidden="true" className="mt-[0.62em] h-px w-3 shrink-0 bg-signal" />
              <span className="min-w-0">{block.text}</span>
            </p>
          );
        }
        return (
          <p key={`${block.kind}-${index}`} className="measure text-[13.5px] leading-relaxed text-ink-2">
            {block.text}
          </p>
        );
      })}
    </div>
  );
}
