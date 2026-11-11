import { useCallback, useEffect, useMemo, useState } from "react";
import { Check, FolderOpen, Plus, Trash2 } from "lucide-react";
import { errorMessage } from "@/lib/http";
import { isNative, native } from "@/lib/native";
import { formatBytes } from "@/lib/format";
import type { InstallLocation, LocationKind, LocationStatus } from "@/lib/types";
import { Badge, Button, Field, Input } from "@/components/ui/controls";
import { Select } from "@/components/ui/select";
import { useToast } from "@/state/toasts";
import { t } from "@/lib/i18n";

const KINDS: LocationKind[] = ["install", "download", "temp"];

const kindLabel = (kind: LocationKind): string =>
  kind === "install" ? t("Install") : kind === "download" ? t("Download") : t("Scratch");

/**
 * Settings → Locations (#99). The panel edits the draft list; "Save settings"
 * persists it. Each row can be checked for writability and free space without
 * saving, so an unplugged drive is obvious before a 60 GB download starts.
 */
export function LocationsPanel({
  locations,
  onChange,
}: {
  locations: InstallLocation[];
  onChange: (locations: InstallLocation[]) => void;
}) {
  const { push } = useToast();
  const nativeAvailable = isNative();
  const [statuses, setStatuses] = useState<Record<string, LocationStatus>>({});
  const [checking, setChecking] = useState<Record<string, boolean>>({});
  const [label, setLabel] = useState("");
  const [path, setPath] = useState("");
  const [kind, setKind] = useState<LocationKind>("install");

  const paths = useMemo(
    () => Array.from(new Set(locations.map((location) => location.path.trim()).filter(Boolean))),
    [locations],
  );
  const pathsKey = paths.join("|");

  const probe = useCallback(
    async (target: string) => {
      if (!nativeAvailable || !target.trim()) return;
      setChecking((current) => ({ ...current, [target]: true }));
      try {
        const status = await native.checkLocation({ path: target.trim() });
        setStatuses((current) => ({ ...current, [target.trim()]: status }));
      } catch (cause) {
        push({ title: t("Could not check this location"), description: errorMessage(cause), tone: "danger" });
      } finally {
        setChecking((current) => ({ ...current, [target]: false }));
      }
    },
    [nativeAvailable, push],
  );

  useEffect(() => {
    if (!nativeAvailable) return;
    let disposed = false;
    // Debounced: typing a path should not spawn one probe per keystroke.
    const timer = window.setTimeout(() => {
      void Promise.all(
        pathsKey
          .split("|")
          .filter(Boolean)
          .map(async (target) => {
            try {
              const status = await native.checkLocation({ path: target });
              if (!disposed) setStatuses((current) => ({ ...current, [target]: status }));
            } catch {
              /* a missing drive is the normal failure here */
            }
          }),
      );
    }, 400);
    return () => {
      disposed = true;
      window.clearTimeout(timer);
    };
  }, [pathsKey, nativeAvailable]);

  const update = (index: number, patch: Partial<InstallLocation>) => {
    onChange(locations.map((location, position) => (position === index ? { ...location, ...patch } : location)));
  };

  const makeDefault = (index: number) => {
    const target = locations[index];
    onChange(
      locations.map((location, position) =>
        position === index
          ? { ...location, default: true }
          : location.kind === target.kind
            ? { ...location, default: false }
            : location,
      ),
    );
  };

  const remove = (index: number) => {
    const target = locations[index];
    if (target.kind === "install" && locations.filter((location) => location.kind === "install").length <= 1) {
      push({ title: t("Keep at least one install location"), tone: "danger" });
      return;
    }
    onChange(locations.filter((_, position) => position !== index));
  };

  const pick = async () => {
    const picked = await native.pickFolder(t("Choose a folder for this location"));
    if (picked) setPath(picked);
    else if (!nativeAvailable) {
      push({ title: t("Folder picker needs the desktop app"), description: t("Type the path instead.") });
    }
  };

  const add = () => {
    if (!label.trim() || !path.trim()) {
      push({ title: t("Name the location and choose a folder"), tone: "danger" });
      return;
    }
    onChange([
      ...locations,
      {
        id: "",
        label: label.trim(),
        path: path.trim(),
        kind,
        // The first location of a kind becomes its default automatically.
        default: !locations.some((location) => location.kind === kind),
      },
    ]);
    void probe(path);
    setLabel("");
    setPath("");
  };

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-baseline justify-between gap-3">
        <div>
          <p className="text-[13px] font-medium text-ink">{t("Locations")}</p>
          <p className="mt-1 max-w-[56ch] text-[11.5px] leading-relaxed text-ink-3">
            {t("Named folders for installs, temporary archives and scratch space. Each install can pick one, and an unplugged drive only fails that job.")}
          </p>
        </div>
        <Badge tone="outline">{locations.length === 1 ? t("1 location") : t("{count} locations", { count: locations.length })}</Badge>
      </div>

      {locations.length === 0 && (
        <p className="border border-dashed border-line px-3 py-4 text-[12px] text-ink-3">
          {t("No location saved yet. Ludex will create one from the default install folder on save.")}
        </p>
      )}

      <div className="flex flex-col divide-y divide-line border-y border-line">
        {locations.map((location, index) => {
          const status = statuses[location.path.trim()];
          const free = status?.freeBytes ?? null;
          const ready = status ? status.writable : null;
          return (
            <div key={`${location.id}-${index}`} className="flex flex-col gap-3 py-4">
              <div className="flex flex-wrap items-start gap-3">
                <div className="min-w-[150px] flex-1">
                  <Input
                    value={location.label}
                    onChange={(event) => update(index, { label: event.target.value })}
                    aria-label={t("Location name")}
                    className="h-9 text-[13px]"
                    maxLength={60}
                  />
                </div>
                <div className="min-w-[220px] flex-[2]">
                  <Input
                    value={location.path}
                    onChange={(event) => update(index, { path: event.target.value })}
                    aria-label={t("Location folder")}
                    spellCheck={false}
                    className="h-9 font-mono text-[11.5px]"
                  />
                </div>
                <div className="w-[130px]">
                  <Select
                    label={t("Location kind")}
                    value={location.kind}
                    onChange={(value) => update(index, { kind: value as LocationKind })}
                    options={KINDS.map((value) => ({ value, label: kindLabel(value) }))}
                    className="h-9 text-[12px]"
                  />
                </div>
                <div className="flex items-center gap-1">
                  {location.default ? (
                    <Badge tone="signal">{t("Default")}</Badge>
                  ) : (
                    <Button size="sm" variant="ghost" onClick={() => makeDefault(index)}>
                      {t("Make default")}
                    </Button>
                  )}
                  <Button
                    size="sm"
                    variant="ghost"
                    icon={<Check className="size-3.5" />}
                    loading={Boolean(checking[location.path.trim()])}
                    onClick={() => void probe(location.path)}
                    disabled={!nativeAvailable}
                  >
                    {t("Check")}
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    aria-label={t("Remove {label}", { label: location.label })}
                    onClick={() => remove(index)}
                    className="text-ink-3 hover:text-danger"
                  >
                    <Trash2 className="size-3.5" />
                  </Button>
                </div>
              </div>

              <div className="flex flex-wrap items-baseline gap-x-4 gap-y-1 font-mono text-[10.5px] text-ink-3">
                <span className="text-ink-2">{kindLabel(location.kind)}</span>
                {status ? (
                  <>
                    <span className={ready ? "text-ok" : "text-danger"}>
                      {ready ? t("Writable") : t("Not writable")}
                    </span>
                    {free !== null && (
                      <span>
                        {t("{free} free of {total}", {
                          free: formatBytes(free),
                          total: status.totalBytes ? formatBytes(status.totalBytes) : "—",
                        })}
                      </span>
                    )}
                    {status.exists ? null : <span className="text-signal-text">{t("Missing")}</span>}
                  </>
                ) : (
                  <span>{nativeAvailable ? t("Not checked") : t("Desktop app only")}</span>
                )}
                {status?.note && <span className="text-ink-2">{status.note}</span>}
              </div>
            </div>
          );
        })}
      </div>

      <div className="grid gap-3 border border-line p-4 sm:grid-cols-[1fr_1.4fr_140px_auto] sm:items-end">
        <Field label={t("Name")} htmlFor="location-name">
          <Input
            id="location-name"
            value={label}
            onChange={(event) => setLabel(event.target.value)}
            placeholder={t("Fast SSD")}
            className="h-9 text-[13px]"
            maxLength={60}
          />
        </Field>
        <Field label={t("Folder")} htmlFor="location-path">
          <div className="flex gap-2">
            <Input
              id="location-path"
              value={path}
              onChange={(event) => setPath(event.target.value)}
              placeholder="/mnt/fast/Games"
              spellCheck={false}
              className="h-9 font-mono text-[11.5px]"
            />
            <Button variant="ghost" onClick={() => void pick()} aria-label={t("Choose folder")} icon={<FolderOpen className="size-3.5" />} />
          </div>
        </Field>
        <Field label={t("Use for")} htmlFor="location-kind">
          <Select
            label={t("Use for")}
            value={kind}
            onChange={(value) => setKind(value as LocationKind)}
            options={KINDS.map((value) => ({ value, label: kindLabel(value) }))}
            className="h-9 text-[12px]"
          />
        </Field>
        <Button variant="default" icon={<Plus className="size-3.5" />} onClick={add} className="h-9">
          {t("Add")}
        </Button>
      </div>
    </div>
  );
}
