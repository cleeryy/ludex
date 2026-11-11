import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Check, Pencil, RefreshCw, X } from "lucide-react";
import { errorMessage } from "@/lib/http";
import { installBridge } from "@/lib/install";
import { formatDate } from "@/lib/format";
import {
  candidateCount,
  confidenceTone,
  isGuess,
  mergeGroup,
  type SaveCandidate,
  type SaveCandidateConfidence,
  type SaveCandidateOverview,
  type SaveCandidateSource,
} from "@/lib/save-discovery";
import { t, tr } from "@/lib/i18n";
import { useToast } from "@/state/toasts";
import { SectionHeader } from "@/components/page";
import { Badge, Button, Input } from "@/components/ui/controls";

const confidenceText = (confidence: SaveCandidateConfidence): string => {
  switch (confidence) {
    case "certain":
      return t("Certain");
    case "likely":
      return t("Likely");
    case "guess":
      return t("Guess");
  }
};

const sourceText = (source: SaveCandidateSource): string => {
  switch (source) {
    case "override":
      return t("User override");
    case "store":
      return t("Store ground truth");
    case "known-folder":
      return t("Known folder");
    case "engine":
      return t("Engine default");
    case "manifest":
      return t("Save database");
    case "manual":
      return t("Added manually");
  }
};

/**
 * Settings → Saves → Save locations (#146).
 *
 * The versioned OS/engine registry lists candidates with their root, provenance
 * and confidence. Nothing here is archived or uploaded: a guess is explicitly
 * marked, and every candidate waits for a confirmation. Confirming only adds a
 * save path; automatic sync stays a separate opt-in on the game page.
 */
export function SaveDiscoveryPanel() {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const [scanning, setScanning] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [editing, setEditing] = useState<Record<string, string>>({});

  const query = useQuery({
    queryKey: ["save-candidates"],
    queryFn: () => installBridge.saveCandidates(),
    retry: false,
    staleTime: 30_000,
  });

  const overview = query.data ?? null;
  const total = candidateCount(overview);

  const scan = async () => {
    setScanning(true);
    try {
      const next = await installBridge.scanSaveCandidates();
      queryClient.setQueryData(["save-candidates"], next);
      push({
        title: t("Scan finished"),
        description:
          next.groups.length === 0
            ? t("No save location candidates were found on this machine.")
            : t("Found {count} candidate(s) across {games} game(s).", {
                count: candidateCount(next),
                games: next.groups.length,
              }),
        tone: "success",
      });
    } catch (cause) {
      push({
        title: t("Could not scan save locations"),
        description: errorMessage(cause),
        tone: "danger",
      });
    } finally {
      setScanning(false);
    }
  };

  const send = async (
    gameId: number,
    candidate: SaveCandidate,
    action: "confirm" | "ignore",
    editedPath?: string,
  ) => {
    const key = `${candidate.path}:${action}`;
    setBusy(key);
    try {
      const group =
        action === "confirm"
          ? await installBridge.confirmSaveCandidate(
              gameId,
              candidate.path,
              editedPath ?? null,
            )
          : await installBridge.ignoreSaveCandidate(gameId, candidate.path);
      queryClient.setQueryData<SaveCandidateOverview | null>(
        ["save-candidates"],
        (previous) => (previous ? mergeGroup(previous, group) : previous),
      );
      setEditing((current) => {
        const next = { ...current };
        delete next[candidate.path];
        return next;
      });
      push({
        title:
          action === "confirm" ? t("Save location confirmed") : t("Save location dismissed"),
        description:
          action === "confirm"
            ? t("The path is now a save path for this game. Automatic sync is unchanged.")
            : t("The candidate will not be shown again."),
        tone: "success",
      });
    } catch (cause) {
      push({
        title: t("Could not update save locations"),
        description: errorMessage(cause),
        tone: "danger",
      });
    } finally {
      setBusy(null);
    }
  };

  return (
    <section aria-labelledby="save-locations-heading">
      <SectionHeader
        title={t("Save locations")}
        description={t(
          "Candidates found by the OS and engine registry, with their root, provenance and confidence. Nothing is archived until you confirm it.",
        )}
        actions={
          <Button
            variant="default"
            size="sm"
            icon={<RefreshCw className="size-3.5" />}
            loading={scanning}
            disabled={scanning}
            onClick={() => void scan()}
          >
            {t("Scan installed games")}
          </Button>
        }
      />
      <div id="save-locations-heading" className="border-t border-line pt-4">
        {overview && (
          <p className="text-[11.5px] leading-relaxed text-ink-3">
            {tr("Registry v{version} · depth ≤{depth} · {entries} entries per folder.", {
              version: overview.registryVersion,
              depth: overview.maxDepth,
              entries: overview.maxEntries,
            })}{" "}
            {t(
              "Paths stay on this machine. A guess is never archived or uploaded automatically.",
            )}
          </p>
        )}

        {query.isError && (
          <p role="alert" className="mt-3 text-[12.5px] text-danger">
            {t("The save location candidates did not load.")} {errorMessage(query.error)}
          </p>
        )}

        {overview?.warnings.length ? (
          <ul className="mt-3 space-y-1 border-l-2 border-line-2 pl-3 text-[11px] leading-relaxed text-ink-3">
            {overview.warnings.map((warning) => (
              <li key={warning}>{warning}</li>
            ))}
          </ul>
        ) : null}

        {query.isPending && <p className="mt-4 text-[12px] text-ink-3">{t("Loading…")}</p>}

        {!query.isPending && total === 0 && (
          <p className="mt-4 max-w-[70ch] text-[12.5px] leading-relaxed text-ink-3">
            {t(
              "No save location candidates yet. Run a scan after installing a game; a confirmed path can also be added manually on its game page.",
            )}
          </p>
        )}

        <div className="mt-4 space-y-6">
          {overview?.groups.map((group) => (
            <div key={group.gameId}>
              <p className="text-[12.5px] font-medium text-ink">{group.title}</p>
              <ul className="mt-2">
                {group.candidates.map((candidate) => (
                  <CandidateRow
                    key={candidate.path}
                    candidate={candidate}
                    editing={editing[candidate.path]}
                    busy={
                      busy === `${candidate.path}:confirm` ||
                      busy === `${candidate.path}:ignore`
                    }
                    onEdit={() =>
                      setEditing((current) => ({
                        ...current,
                        [candidate.path]: candidate.path,
                      }))
                    }
                    onCancelEdit={() =>
                      setEditing((current) => {
                        const next = { ...current };
                        delete next[candidate.path];
                        return next;
                      })
                    }
                    onChangeEdit={(value) =>
                      setEditing((current) => ({
                        ...current,
                        [candidate.path]: value,
                      }))
                    }
                    onConfirm={(editedPath) =>
                      void send(group.gameId, candidate, "confirm", editedPath)
                    }
                    onIgnore={() => void send(group.gameId, candidate, "ignore")}
                  />
                ))}
              </ul>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}

function CandidateRow({
  candidate,
  editing,
  busy,
  onEdit,
  onCancelEdit,
  onChangeEdit,
  onConfirm,
  onIgnore,
}: {
  candidate: SaveCandidate;
  editing: string | undefined;
  busy: boolean;
  onEdit: () => void;
  onCancelEdit: () => void;
  onChangeEdit: (value: string) => void;
  onConfirm: (editedPath?: string) => void;
  onIgnore: () => void;
}) {
  const guess = isGuess(candidate);
  return (
    <li
      className={`border-b border-line py-3 ${
        guess ? "border-l-2 border-l-signal/60 pl-3" : "pl-3"
      }`}
    >
      <div className="flex flex-wrap items-center gap-2">
        <Badge tone={confidenceTone(candidate.confidence)}>
          {confidenceText(candidate.confidence)}
        </Badge>
        <Badge tone="outline">{sourceText(candidate.source)}</Badge>
        <span className="font-mono text-[10px] text-ink-3">{candidate.rootId}</span>
        {candidate.confirmed && <Badge tone="ok">{t("Confirmed")}</Badge>}
        {guess && (
          <span className="text-[10.5px] text-ink-3">
            {t("Guessed from an engine default; check the path before confirming.")}
          </span>
        )}
      </div>

      {editing === undefined ? (
        <p className="mt-1.5 break-all font-mono text-[11px] text-ink-2">{candidate.path}</p>
      ) : (
        <div className="mt-2 flex flex-wrap items-center gap-2">
          <Input
            value={editing}
            onChange={(event) => onChangeEdit(event.target.value)}
            className="h-9 min-w-[280px] flex-1 font-mono text-[11.5px]"
            aria-label={t("Save path")}
          />
          <Button variant="primary" size="sm" loading={busy} onClick={() => onConfirm(editing)}>
            {t("Save")}
          </Button>
          <Button variant="ghost" size="sm" onClick={onCancelEdit}>
            {t("Cancel")}
          </Button>
        </div>
      )}

      <p className="mt-1 text-[11px] leading-relaxed text-ink-3">{candidate.evidence}</p>
      <p className="mt-1 text-[10.5px] text-ink-3">
        {candidate.subpath ? `${candidate.subpath} · ` : ""}
        {candidate.firstSeen
          ? tr("First seen {date}", { date: formatDate(candidate.firstSeen) })
          : t("First seen unknown")}
      </p>

      {!candidate.confirmed && editing === undefined && (
        <div className="mt-2 flex flex-wrap items-center gap-2">
          <Button
            variant="default"
            size="sm"
            icon={<Check className="size-3.5" />}
            loading={busy}
            onClick={() => onConfirm()}
          >
            {guess ? t("Confirm guess") : t("Confirm")}
          </Button>
          <Button variant="ghost" size="sm" icon={<Pencil className="size-3.5" />} onClick={onEdit}>
            {t("Edit path")}
          </Button>
          <Button variant="ghost" size="sm" icon={<X className="size-3.5" />} onClick={onIgnore}>
            {t("Ignore")}
          </Button>
        </div>
      )}
    </li>
  );
}
