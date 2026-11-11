import type { UpdateStatus } from "@/lib/types";

/** Channel followed by the updater. */
export type UpdateChannel = UpdateStatus["channel"];

/**
 * Update center helpers (#163).
 *
 * The decisions live here, outside React, so they can be unit-tested:
 * - whether an available version should raise a prompt or a badge (skip and
 *   "remind me later" both silence it, but the update stays installable);
 * - whether switching to a channel needs an explicit warning;
 * - how release notes are turned into safe text blocks.
 *
 * Release notes are rendered as React nodes built from these blocks, never as
 * HTML, so a release body cannot inject markup or scripts into the app.
 */

/** Release list, used when no version-specific page is known. */
export const RELEASES_URL = "https://github.com/cleeryy/ludex/releases";/** Rolling early-access release. */
export const EDGE_RELEASE_URL = `${RELEASES_URL}/tag/edge`;
/** localStorage key holding the end of a "remind me later" window. */
export const REMIND_LATER_STORAGE_KEY = "ludex.update.remindAfter";
/** localStorage key remembering the version whose launch prompt was closed. */
export const PROMPTED_STORAGE_KEY = "ludex.update.promptedVersion";
/** "Remind me later" silences the startup prompt for a day. */
export const REMIND_LATER_MS = 24 * 60 * 60 * 1000;
/** Notes are capped before parsing: a release body is never a document. */
const MAX_NOTES_CHARS = 20_000;


/** Release page for a channel and version, mirroring `updater::release_url_for`. */
export function releaseUrl(channel: UpdateChannel, version?: string | null): string {
  if (channel === "early") return EDGE_RELEASE_URL;
  const tag = (version ?? "").trim().replace(/^v/, "");
  return tag ? `${RELEASES_URL}/tag/v${tag}` : `${RELEASES_URL}/latest`;
}

/** True for `edge` versions such as `0.3.2-edge.179`. */
export function isPrerelease(version: string): boolean {
  return version.includes("-");
}

/** CI run number of an `edge` build, when the version carries one. */
export function edgeBuildNumber(version: string): number | null {
  const marker = "-edge.";
  const index = version.indexOf(marker);
  if (index === -1) return null;
  const build = Number(version.slice(index + marker.length));
  return Number.isInteger(build) && build > 0 ? build : null;
}

/** True when `available` is exactly the version the user skipped. */
export function isVersionSkipped(
  available: string | null | undefined,
  skipped: string | null | undefined,
): boolean {
  const version = available?.trim();
  const skip = skipped?.trim();
  return Boolean(version && skip && version === skip);
}

/** Whether an available update should raise a prompt or a badge at all. */
export function shouldNotify(
  available: string | null | undefined,
  skipped: string | null | undefined,
): boolean {
  return Boolean(available?.trim()) && !isVersionSkipped(available, skipped);
}

/**
 * Whether the startup prompt should open.
 *
 * A version already prompted (its dialog was closed), a skipped version and an
 * active "remind me later" window all keep it closed. The update center still
 * lists the update in every case.
 */
export function shouldPrompt({
  status,
  skippedVersion,
  remindAfter,
  promptedVersion,
  now = Date.now(),
}: {
  status: Pick<UpdateStatus, "available" | "notify"> | null | undefined;
  skippedVersion: string | null | undefined;
  remindAfter: number | null;
  promptedVersion: string | null | undefined;
  now?: number;
}): boolean {
  const available = status?.available;
  if (!available) return false;
  if (status.notify === false) return false;
  if (isVersionSkipped(available.version, skippedVersion)) return false;
  if (promptedVersion && promptedVersion.trim() === available.version) return false;
  if (remindAfter !== null && Number.isFinite(remindAfter) && now < remindAfter) return false;
  return true;
}

/** Whether the persistent "update available" badge should be visible. */
export function shouldShowBadge(status: UpdateStatus | null | undefined): boolean {
  return Boolean(status?.available) && status?.notify === true;
}

/**
 * Switching to early access needs an explicit warning; going back to stable
 * does not, and re-selecting the current channel is a no-op.
 */
export function channelNeedsConfirmation(
  current: UpdateChannel,
  next: UpdateChannel,
): boolean {
  return next !== current && next === "early";
}

export type NoteBlockKind = "heading" | "paragraph" | "bullet";

export interface NoteBlock {
  kind: NoteBlockKind;
  text: string;
}

/**
 * Reduce inline Markdown to plain text. Links keep their label (the release
 * page is the canonical, clickable copy), emphasis markers and code fences are
 * dropped, and tag-like sequences are removed so no markup survives.
 */
export function stripInlineMarkdown(line: string): string {
  return line
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
    .replace(/`([^`]+)`/g, "$1")
    .replace(/\*\*([^*]+)\*\*/g, "$1")
    .replace(/__([^_]+)__/g, "$1")
    .replace(/(^|[\s(])[*_]([^*_\n]+)[*_](?=[\s.,;:!?)]|$)/g, "$1$2")
    .replace(/<[^>]*>/g, "")
    // GitHub release bodies are HTML-escaped in places ("Tom &amp; Jerry").
    .replace(/&amp;/g, "&")
    .replace(/&quot;/g, '"')
    .replace(/&#39;|&apos;/g, "'")
    .replace(/&nbsp;/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}

/**
 * Parse a release body into headings, bullets and paragraphs.
 *
 * Blockquotes become paragraphs, horizontal rules and code-fence markers are
 * dropped, and the result is bounded so a giant release body cannot stall the
 * screen. Returns `[]` when the body is empty (the UI then explains that the
 * build has no notes, as happens for automatic `edge` releases).
 */
export function releaseNoteBlocks(notes: string | null | undefined): NoteBlock[] {
  const source = (notes ?? "").replace(/\r\n?/g, "\n").slice(0, MAX_NOTES_CHARS);
  if (!source.trim()) return [];

  const blocks: NoteBlock[] = [];
  let paragraph: string[] = [];
  let inCode = false;

  const flush = () => {
    if (!paragraph.length) return;
    const text = stripInlineMarkdown(paragraph.join(" "));
    paragraph = [];
    if (text) blocks.push({ kind: "paragraph", text });
  };

  for (const rawLine of source.split("\n")) {
    const line = rawLine.trim();
    if (/^```/.test(line)) {
      flush();
      inCode = !inCode;
      continue;
    }
    if (!line) {
      flush();
      continue;
    }
    if (inCode) {
      paragraph.push(line);
      continue;
    }
    const heading = /^(#{1,6})\s+(.*)$/.exec(line);
    if (heading) {
      flush();
      const text = stripInlineMarkdown(heading[2]);
      if (text) blocks.push({ kind: "heading", text });
      continue;
    }
    const bullet = /^[-*+]\s+(.*)$/.exec(line);
    if (bullet) {
      flush();
      const text = stripInlineMarkdown(bullet[1]);
      if (text) blocks.push({ kind: "bullet", text });
      continue;
    }
    if (/^([-*_])\1{2,}$/.test(line)) {
      flush();
      continue;
    }
    paragraph.push(line.replace(/^>\s?/, ""));
  }
  flush();

  return blocks.slice(0, 200);
}

/** End of the active "remind me later" window, or null. */
export function readRemindAfter(now = Date.now()): number | null {
  try {
    const raw = localStorage.getItem(REMIND_LATER_STORAGE_KEY);
    if (!raw) return null;
    const value = Number(raw);
    if (!Number.isFinite(value) || value <= now) return null;
    return value;
  } catch {
    return null;
  }
}

/** Start (or extend) a "remind me later" window and return its end. */
export function setRemindLater(now = Date.now(), delay = REMIND_LATER_MS): number {
  const until = now + delay;
  try {
    localStorage.setItem(REMIND_LATER_STORAGE_KEY, String(until));
  } catch {
    /* private mode or full storage: the in-memory state still works */
  }
  return until;
}

export function clearRemindLater(): void {
  try {
    localStorage.removeItem(REMIND_LATER_STORAGE_KEY);
  } catch {
    /* ignore */
  }
}

/**
 * Version whose launch prompt the user already closed.
 *
 * Persisted (not per run): a version prompts once. "Remind me later" is the
 * explicit way to see it again; "Skip this version" silences the version
 * entirely.
 */
export function readPromptedVersion(): string | null {
  try {
    return localStorage.getItem(PROMPTED_STORAGE_KEY);
  } catch {
    return null;
  }
}

export function markPrompted(version: string): void {
  try {
    localStorage.setItem(PROMPTED_STORAGE_KEY, version);
  } catch {
    /* ignore */
  }
}
