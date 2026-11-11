import type { ConfigFileInfo } from "./types";

/**
 * Pure helpers behind the generic config file list (#147).
 *
 * The list is name-agnostic on purpose: the backend returns every supported
 * text config under the install folder, and the UI only decides what to show
 * and whether a file may be read. Keeping this out of the component makes the
 * read-only rules testable without a DOM.
 */

/** Case-insensitive search over the relative path. Empty query keeps the order. */
export function filterConfigFiles(files: ConfigFileInfo[], query: string): ConfigFileInfo[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return files;
  return files.filter((file) => file.path.toLowerCase().includes(needle));
}

/**
 * Whether `read_game_config` may be called for this entry. Read-only entries
 * must never trigger a query: the listing already carries the reason, and a
 * second failure would surface as a generic error (#147).
 */
export function canLoadConfigFile(file: ConfigFileInfo | undefined): boolean {
  return Boolean(file?.editable);
}

/** Reason to show for a read-only file, or `null` when it is editable. */
export function readOnlyReason(file: ConfigFileInfo): string | null {
  if (file.editable) return null;
  const note = file.note?.trim();
  return note ? note : null;
}

/** First editable file, else the first entry, else an empty selection. */
export function defaultConfigPath(files: ConfigFileInfo[]): string {
  return files.find((file) => file.editable)?.path ?? files[0]?.path ?? "";
}
