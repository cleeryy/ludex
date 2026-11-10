import { useEffect, useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Plus, Trash2 } from "lucide-react";
import { api } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { defaultOperator, defaultRule, evaluateRules, operatorsFor, smartFields } from "@/lib/collections";
import { formatYear } from "@/lib/format";
import type { Collection, CollectionKind, SmartField, SmartRule, SmartRules } from "@/lib/types";
import { useToast } from "@/state/toasts";
import { useAllGames } from "@/lib/hooks";
import { Badge, Button, Checkbox, Field, Input, Segmented, Switch, Textarea } from "@/components/ui/controls";
import { Dialog } from "@/components/ui/dialog";
import { Select } from "@/components/ui/select";
import { t } from "@/lib/i18n";

const EMPTY_RULES: SmartRules = { match: "all", rules: [] };

export function CollectionDialog({
  open,
  onOpenChange,
  collection,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  collection?: Collection | null;
}) {
  const queryClient = useQueryClient();
  const { push } = useToast();
  const allGames = useAllGames();
  const games = allGames.data?.items ?? [];
  const editing = Boolean(collection);

  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [isPublic, setIsPublic] = useState(false);
  const [kind, setKind] = useState<CollectionKind>("manual");
  const [rules, setRules] = useState<SmartRules>(EMPTY_RULES);
  const [selected, setSelected] = useState<number[]>([]);
  const [search, setSearch] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const members = useQuery({
    queryKey: ["collection", collection?.id, "games"],
    queryFn: () => api.collectionGames(collection!.id, 1, 200),
    enabled: open && editing && collection?.kind === "manual",
  });

  useEffect(() => {
    if (!open) return;
    setName(collection?.name ?? "");
    setDescription(collection?.description ?? "");
    setIsPublic(collection?.isPublic ?? false);
    setKind(collection?.kind ?? "manual");
    setRules(collection?.rules ?? EMPTY_RULES);
    setSelected([]);
    setSearch("");
    setError(null);
  }, [open, collection]);

  useEffect(() => {
    if (members.data) setSelected(members.data.items.map((game) => game.id));
  }, [members.data]);

  const filtered = useMemo(() => {
    const query = search.trim().toLowerCase();
    if (!query) return games;
    return games.filter((game) => game.title.toLowerCase().includes(query));
  }, [games, search]);

  const previewCount = useMemo(
    () => (kind === "smart" ? games.filter((game) => evaluateRules(rules, game)).length : 0),
    [games, kind, rules],
  );

  const patchRule = (index: number, patch: Partial<SmartRule>) => {
    setRules((current) => ({
      ...current,
      rules: current.rules.map((rule, position) => (position === index ? { ...rule, ...patch } : rule)),
    }));
  };

  const changeField = (index: number, field: SmartField) => {
    setRules((current) => ({
      ...current,
      rules: current.rules.map((rule, position) => (position === index ? defaultRule(field) : rule)),
    }));
  };

  const save = async () => {
    setError(null);
    if (!name.trim()) {
      setError(t("Give the collection a name."));
      return;
    }
    if (kind === "smart" && rules.rules.some((rule) => rule.value === "" || rule.value === null)) {
      setError(t("Every rule needs a value."));
      return;
    }
    setBusy(true);
    try {
      if (collection) {
        await api.updateCollection(collection.id, {
          name: name.trim(),
          description: description.trim() || null,
          isPublic,
          kind,
          rules: kind === "smart" ? rules : undefined,
        });
        if (kind === "manual") {
          const before = new Set(members.data?.items.map((game) => game.id) ?? []);
          const next = new Set(selected);
          const toAdd = [...next].filter((id) => !before.has(id));
          const toRemove = [...before].filter((id) => !next.has(id));
          if (toAdd.length) await api.addCollectionGames(collection.id, toAdd);
          for (const id of toRemove) await api.removeCollectionGame(collection.id, id);
        }
        await queryClient.invalidateQueries({ queryKey: ["collections"] });
        await queryClient.invalidateQueries({ queryKey: ["collection", collection.id] });
        await queryClient.invalidateQueries({ queryKey: ["collection", collection.id, "games"] });
        push({ title: t("Collection updated"), tone: "success" });
      } else {
        const created = await api.createCollection({
          name: name.trim(),
          description: description.trim() || null,
          isPublic,
          kind,
          rules: kind === "smart" ? rules : undefined,
          gameIds: kind === "manual" ? selected : undefined,
        });
        await queryClient.invalidateQueries({ queryKey: ["collections"] });
        push({ title: t("Collection created"), description: created.name, tone: "success" });
      }
      onOpenChange(false);
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title={editing ? t("Edit collection") : t("New collection")}
      description={
        editing
          ? t("Rename it, change who can see it, or switch between a hand-picked shelf and a rule.")
          : t("A manual shelf holds the games you pick. A smart shelf fills itself from rules.")
      }
      size="lg"
      footer={
        <>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            {t("Cancel")}
          </Button>
          <Button variant="primary" loading={busy} onClick={() => void save()}>
            {editing ? t("Save collection") : t("Create collection")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-5">
        <div className="grid gap-4 sm:grid-cols-[1fr_auto]">
          <Field label={t("Name")} htmlFor="collection-name">
            <Input
              id="collection-name"
              value={name}
              onChange={(event) => setName(event.target.value)}
              placeholder={t("Friday night co-op")}
              maxLength={80}
            />
          </Field>
          <Field label={t("Visibility")} hint={isPublic ? t("Anyone on the server can read it") : t("Only you")}>
            <div className="flex h-11 items-center gap-2">
              <Switch checked={isPublic} onCheckedChange={setIsPublic} label={t("Public collection")} />
              <Badge tone={isPublic ? "signal" : "outline"}>{isPublic ? t("Public") : t("Private")}</Badge>
            </div>
          </Field>
        </div>

        <Field label={t("Description")} htmlFor="collection-description" hint={t("Optional. One line is enough.")}>
          <Textarea
            id="collection-description"
            value={description}
            onChange={(event) => setDescription(event.target.value)}
            placeholder={t("Two controllers, one couch, no excuses.")}
            rows={2}
            maxLength={2000}
            className="min-h-[72px]"
          />
        </Field>

        <div>
          <p className="pb-2 text-[13px] font-medium text-ink">{t("Kind")}</p>
          <Segmented
            value={kind}
            onChange={setKind}
            label={t("Collection kind")}
            options={[
              { value: "manual", label: t("Manual shelf") },
              { value: "smart", label: t("Smart list") },
            ]}
          />
          <p className="mt-2 text-[11.5px] leading-relaxed text-ink-3">
            {kind === "manual"
              ? t("Pick games by hand. You can add or remove them later from the collection page.")
              : t("Rules are evaluated when the collection is opened, against your library.")}
          </p>
        </div>

        {kind === "manual" ? (
          <div>
            <div className="flex flex-wrap items-baseline justify-between gap-2 pb-2">
              <p className="text-[13px] font-medium text-ink">{t("Games")}</p>
              <span className="num text-[11.5px] text-ink-3">
                {t("{selected} of {total} selected", { selected: selected.length, total: games.length })}
              </span>
            </div>
            <Input
              value={search}
              onChange={(event) => setSearch(event.target.value)}
              placeholder={t("Filter by title")}
              aria-label={t("Filter games")}
              className="mb-2 h-9 text-[13px]"
            />
            <div className="max-h-[260px] overflow-y-auto border border-line">
              {filtered.length === 0 ? (
                <p className="p-3 text-[12.5px] text-ink-3">{t("No games match that filter.")}</p>
              ) : (
                filtered.map((game) => {
                  const checked = selected.includes(game.id);
                  return (
                    <div
                      key={game.id}
                      className="flex items-center gap-3 border-b border-line px-3 py-2 last:border-0"
                    >
                      <Checkbox
                        checked={checked}
                        onCheckedChange={() =>
                          setSelected((current) =>
                            checked ? current.filter((id) => id !== game.id) : [...current, game.id],
                          )
                        }
                        label={`Include ${game.title}`}
                      />
                      <span className="min-w-0 flex-1">
                        <span className="block truncate text-[13px] text-ink">{game.title}</span>
                        <span className="mt-0.5 block font-mono text-[10px] text-ink-3">
                          {formatYear(game.releaseDate)} · {game.genres.join(", ") || t("Unclassified")}
                        </span>
                      </span>
                    </div>
                  );
                })
              )}
            </div>
          </div>
        ) : (
          <div>
            <div className="flex flex-wrap items-center justify-between gap-3 pb-2">
              <p className="text-[13px] font-medium text-ink">{t("Rules")}</p>
              <div className="flex items-center gap-2">
                <span className="text-[11.5px] text-ink-3">{t("Match")}</span>
                <Segmented
                  value={rules.match}
                  onChange={(match) => setRules((current) => ({ ...current, match }))}
                  label={t("Match mode")}
                  size="sm"
                  options={[
                    { value: "all", label: t("All") },
                    { value: "any", label: t("Any") },
                  ]}
                />
              </div>
            </div>

            <div className="flex flex-col gap-2">
              {rules.rules.map((rule, index) => {
                const operators = operatorsFor(rule.field);
                const numeric = ["year", "players", "playtime", "rating"].includes(rule.field);
                return (
                  <div key={index} className="grid gap-2 border border-line p-2 sm:grid-cols-[1fr_120px_1fr_auto]">
                    <Select
                      label={`Rule ${index + 1} field`}
                      value={rule.field}
                      onChange={(field) => changeField(index, field)}
                      options={smartFields().map((field) => ({ value: field.value, label: field.label }))}
                      className="h-9 text-[12.5px]"
                    />
                    <Select
                      label={`Rule ${index + 1} operator`}
                      value={rule.op ?? defaultOperator(rule.field)}
                      onChange={(op) => patchRule(index, { op })}
                      options={operators.map((operator) => ({ value: operator.value, label: operator.label }))}
                      className="h-9 text-[12.5px]"
                    />
                    {rule.field === "installed" ? (
                      <div className="flex h-9 items-center gap-2">
                        <Switch
                          checked={Boolean(rule.value)}
                          onCheckedChange={(value) => patchRule(index, { value })}
                          label={`Rule ${index + 1} installed`}
                        />
                        <span className="text-[12.5px] text-ink-2">{rule.value ? t("Available") : t("Not available")}</span>
                      </div>
                    ) : (
                      <div className="flex gap-2">
                        <Input
                          type={numeric ? "number" : "text"}
                          value={String(rule.value ?? "")}
                          onChange={(event) =>
                            patchRule(index, { value: numeric ? Number(event.target.value) : event.target.value })
                          }
                          aria-label={`Rule ${index + 1} value`}
                          className="h-9 text-[12.5px]"
                        />
                        {rule.op === "between" && (
                          <Input
                            type="number"
                            value={String(rule.value2 ?? "")}
                            onChange={(event) => patchRule(index, { value2: Number(event.target.value) })}
                            aria-label={`Rule ${index + 1} upper bound`}
                            className="h-9 text-[12.5px]"
                          />
                        )}
                      </div>
                    )}
                    <Button
                      variant="ghost"
                      size="sm"
                      className="h-9 w-9 px-0 text-ink-3"
                      aria-label={`Remove rule ${index + 1}`}
                      icon={<Trash2 className="size-3.5" />}
                      onClick={() =>
                        setRules((current) => ({
                          ...current,
                          rules: current.rules.filter((_, position) => position !== index),
                        }))
                      }
                    />
                  </div>
                );
              })}
            </div>

            <div className="mt-2 flex flex-wrap items-center justify-between gap-3">
              <Button
                variant="default"
                size="sm"
                icon={<Plus className="size-3.5" />}
                onClick={() =>
                  setRules((current) => ({ ...current, rules: [...current.rules, defaultRule("genre")] }))
                }
              >
                {t("Add rule")}
              </Button>
              <span className="num text-[11.5px] text-ink-3">
                {t("{shown} of {total} games", { shown: previewCount, total: games.length })}
              </span>
            </div>
            {rules.rules.length === 0 && (
              <p className="mt-2 border-l-2 border-line-2 pl-3 text-[11.5px] leading-relaxed text-ink-3">
                {t("A smart collection with no rules stays empty. Add at least one.")}
              </p>
            )}
          </div>
        )}

        {error && (
          <p role="alert" className="border-l-2 border-danger pl-3 text-[13px] text-danger">
            {error}
          </p>
        )}
      </div>
    </Dialog>
  );
}
