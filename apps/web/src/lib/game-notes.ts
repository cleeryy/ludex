/**
 * Per-game personal notes (#349).
 *
 * Stored in server-scoped localStorage so players can jot down mod load order,
 * puzzle codes, controller reminders, or where they left off without requiring
 * a server schema change.
 */

import { scopedStorageKey } from "@/lib/server-storage";

export const GAME_NOTES_STORAGE_KEY = "ludex.game.notes";
export const MAX_GAME_NOTE_LENGTH = 4000;

export interface GameNoteRecord {
  body: string;
  updatedAt: string;
}

type NotesStore = Record<string, GameNoteRecord>;

function storageKey(): string {
  return scopedStorageKey(GAME_NOTES_STORAGE_KEY);
}

function readStore(): NotesStore {
  try {
    const raw = localStorage.getItem(storageKey());
    if (!raw) return {};
    const parsed = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};
    const out: NotesStore = {};
    for (const [key, value] of Object.entries(parsed as Record<string, unknown>)) {
      if (!value || typeof value !== "object") continue;
      const body = typeof (value as { body?: unknown }).body === "string"
        ? (value as { body: string }).body.slice(0, MAX_GAME_NOTE_LENGTH)
        : "";
      const updatedAt = typeof (value as { updatedAt?: unknown }).updatedAt === "string"
        ? (value as { updatedAt: string }).updatedAt
        : "";
      if (body.trim()) {
        out[key] = { body, updatedAt };
      }
    }
    return out;
  } catch {
    return {};
  }
}

function writeStore(store: NotesStore): void {
  try {
    if (Object.keys(store).length === 0) {
      localStorage.removeItem(storageKey());
    } else {
      localStorage.setItem(storageKey(), JSON.stringify(store));
    }
  } catch {
    // storage may be disabled in private browsing
  }
}

export function loadGameNote(gameId: number): GameNoteRecord | null {
  const store = readStore();
  return store[String(gameId)] ?? null;
}

export function saveGameNote(gameId: number, body: string, now = new Date()): GameNoteRecord | null {
  const store = readStore();
  const key = String(gameId);
  const clamped = body.slice(0, MAX_GAME_NOTE_LENGTH);
  if (!clamped.trim()) {
    delete store[key];
    writeStore(store);
    return null;
  }
  const record: GameNoteRecord = { body: clamped, updatedAt: now.toISOString() };
  store[key] = record;
  writeStore(store);
  return record;
}

export function clearGameNote(gameId: number): void {
  const store = readStore();
  delete store[String(gameId)];
  writeStore(store);
}

export function listNotedGameIds(): number[] {
  return Object.keys(readStore())
    .map((key) => Number(key))
    .filter((id) => Number.isFinite(id) && id > 0);
}
