import { useEffect, useRef, useState } from "react";
import { Trash2 } from "lucide-react";
import {
  clearGameNote,
  loadGameNote,
  MAX_GAME_NOTE_LENGTH,
  saveGameNote,
} from "@/lib/game-notes";
import { formatRelative } from "@/lib/format";
import { SettingsGroup } from "@/components/settings/settings-section";
import { Button, Textarea } from "@/components/ui/controls";
import { t } from "@/lib/i18n";

const AUTOSAVE_DELAY_MS = 400;

/**
 * Private per-game notepad on the Game Overview tab (#349). Autosaves after a
 * brief pause or on blur so players never lose a note while switching tabs.
 */
export function GameNotesPanel({ gameId }: { gameId: number }) {
  const [state, setState] = useState<{
    gameId: number;
    draft: string;
    updatedAt: string | null;
    dirty: boolean;
  }>(() => {
    const existing = loadGameNote(gameId);
    return {
      gameId,
      draft: existing?.body ?? "",
      updatedAt: existing?.updatedAt ?? null,
      dirty: false,
    };
  });

  // Reset when navigating between games.
  const active = state.gameId === gameId
    ? state
    : (() => {
        const existing = loadGameNote(gameId);
        return {
          gameId,
          draft: existing?.body ?? "",
          updatedAt: existing?.updatedAt ?? null,
          dirty: false,
        };
      })();

  const latestRef = useRef(active);
  latestRef.current = active;

  const commit = (targetGameId: number, value: string) => {
    const saved = saveGameNote(targetGameId, value);
    setState((current) =>
      current.gameId === targetGameId
        ? {
            gameId: targetGameId,
            draft: saved ? value.slice(0, MAX_GAME_NOTE_LENGTH) : "",
            updatedAt: saved?.updatedAt ?? null,
            dirty: false,
          }
        : current,
    );
  };

  useEffect(() => {
    if (!active.dirty) return;
    const timer = window.setTimeout(() => {
      commit(active.gameId, active.draft);
    }, AUTOSAVE_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, [active.dirty, active.draft, active.gameId]);

  useEffect(() => {
    return () => {
      if (latestRef.current.dirty) {
        saveGameNote(latestRef.current.gameId, latestRef.current.draft);
      }
    };
  }, [gameId]);

  const handleClear = () => {
    clearGameNote(gameId);
    setState({ gameId, draft: "", updatedAt: null, dirty: false });
  };

  const count = active.draft.length;

  return (
    <SettingsGroup
      title={t("Personal notes")}
      description={t("Private notes kept on this device — build ideas, mod order, puzzle codes or where you left off.")}
    >
      <label htmlFor={`game-notes-${gameId}`} className="sr-only">
        {t("Personal notes")}
      </label>
      <Textarea
        id={`game-notes-${gameId}`}
        value={active.draft}
        maxLength={MAX_GAME_NOTE_LENGTH}
        rows={4}
        placeholder={t("Write a note for this game…")}
        onChange={(event) =>
          setState({
            gameId,
            draft: event.target.value,
            updatedAt: active.updatedAt,
            dirty: true,
          })
        }
        onBlur={() => {
          if (active.dirty) commit(gameId, active.draft);
        }}
        className="min-h-[104px] text-body leading-relaxed"
      />
      <div className="mt-2 flex flex-wrap items-center justify-between gap-2 text-meta text-ink-3">
        <span>
          {active.dirty
            ? t("Saving…")
            : active.updatedAt
              ? t("Saved {when}", { when: formatRelative(active.updatedAt) })
              : t("Stored locally on this device.")}
        </span>
        <div className="flex items-center gap-3">
          {count > 0 && (
            <span className="num">
              {count} / {MAX_GAME_NOTE_LENGTH}
            </span>
          )}
          {active.draft.trim().length > 0 && (
            <Button
              variant="ghost"
              size="sm"
              icon={<Trash2 className="size-3.5" />}
              onClick={handleClear}
            >
              {t("Clear note")}
            </Button>
          )}
        </div>
      </div>
    </SettingsGroup>
  );
}
