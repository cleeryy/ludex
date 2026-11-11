import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Cpu, Plus, Trash2 } from "lucide-react";
import { errorMessage } from "@/lib/http";
import { native } from "@/lib/native";
import type { RomEmulator } from "@/lib/types";
import { useToast } from "@/state/toasts";
import { Badge, Button, Field, Input } from "@/components/ui/controls";
import { Select } from "@/components/ui/select";
import { t } from "@/lib/i18n";

/**
 * Desktop emulator registry (#52). Everything here is local to the machine:
 * the commands point at the user's own RetroArch/standalone binaries and no
 * binary is bundled with Ludex.
 */
export function RomEmulatorsPanel() {
  const queryClient = useQueryClient();
  const { push } = useToast();
  const settings = useQuery({ queryKey: ["rom-settings"], queryFn: () => native.getRomSettings() });
  const [draft, setDraft] = useState<RomEmulator>({ id: "", name: "", command: "", platforms: [], enabled: true, builtin: false });
  const [platforms, setPlatforms] = useState("");
  const [validation, setValidation] = useState<{ valid: boolean; error?: string | null } | null>(null);

  useEffect(() => {
    if (!settings.data) return;
    setDraft((current) => (current.id ? current : { ...current, platforms: [] }));
  }, [settings.data]);

  const refresh = async () => {
    await queryClient.invalidateQueries({ queryKey: ["rom-settings"] });
  };

  const save = async (emulator: RomEmulator) => {
    try {
      await native.saveRomEmulator(emulator);
      await refresh();
      push({ title: t("Emulator saved"), tone: "success" });
      setDraft({ id: "", name: "", command: "", platforms: [], enabled: true, builtin: false });
      setPlatforms("");
    } catch (cause) {
      push({ title: t("Could not save the emulator"), description: errorMessage(cause), tone: "danger" });
    }
  };

  const remove = async (emulator: RomEmulator) => {
    try {
      await native.deleteRomEmulator(emulator.id);
      await refresh();
    } catch (cause) {
      push({ title: t("Could not delete the emulator"), description: errorMessage(cause), tone: "danger" });
    }
  };

  const setDefault = async (platform: string, emulatorId: string) => {
    try {
      await native.setRomDefault(platform, emulatorId || null);
      await refresh();
    } catch (cause) {
      push({ title: t("Could not set the default"), description: errorMessage(cause), tone: "danger" });
    }
  };

  const validate = async () => {
    try {
      const result = await native.validateRomCommand(draft.command);
      setValidation(result);
    } catch (cause) {
      setValidation({ valid: false, error: errorMessage(cause) });
    }
  };

  const data = settings.data;
  if (!data) {
    return <p className="text-[12.5px] text-ink-3">{t("The emulator registry is available in the desktop app.")}</p>;
  }

  return (
    <div className="flex flex-col gap-5">
      <div className="overflow-x-auto">
        <table className="w-full border-collapse text-[12.5px]">
          <thead>
            <tr className="border-b border-line text-left font-mono text-[10px] uppercase tracking-[0.1em] text-ink-3">
              <th className="py-2 font-normal">{t("Emulator")}</th>
              <th className="py-2 font-normal">{t("Command")}</th>
              <th className="py-2 font-normal">{t("Platforms")}</th>
              <th className="py-2 font-normal" />
            </tr>
          </thead>
          <tbody>
            {data.registry.emulators.map((emulator) => (
              <tr key={emulator.id} className="border-b border-line/70">
                <td className="py-2 pr-3 text-ink">
                  <span className="flex items-center gap-2">
                    {emulator.name}
                    {emulator.builtin && <Badge tone="outline">{t("Preset")}</Badge>}
                    {!emulator.enabled && <Badge tone="danger">{t("Disabled")}</Badge>}
                  </span>
                </td>
                <td className="max-w-[280px] truncate py-2 pr-3 font-mono text-[10.5px] text-ink-3" title={emulator.command}>
                  {emulator.command}
                </td>
                <td className="py-2 pr-3 font-mono text-[10.5px] text-ink-3">{emulator.platforms.join(", ") || "—"}</td>
                <td className="py-2 text-right">
                  <Button size="sm" variant="ghost" onClick={() => setDraft({ ...emulator })}>
                    {t("Edit")}
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    aria-label={t("Delete {name}", { name: emulator.name })}
                    onClick={() => void remove(emulator)}
                  >
                    <Trash2 className="size-3.5" />
                  </Button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      <div className="rounded-[2px] border border-line/70 p-3">
        <p className="flex items-center gap-2 font-mono text-[10px] uppercase tracking-[0.1em] text-ink-3">
          <Cpu className="size-3.5" /> {draft.id ? t("Edit emulator") : t("Add an emulator")}
        </p>
        <div className="mt-3 grid gap-3 sm:grid-cols-2">
          <Field label={t("Name")}>
            <Input value={draft.name} onChange={(event) => setDraft({ ...draft, name: event.target.value, id: draft.id || slug(event.target.value) })} />
          </Field>
          <Field label={t("Platforms (comma separated)")}>
            <Input
              value={platforms || draft.platforms.join(", ")}
              onChange={(event) => {
                setPlatforms(event.target.value);
                setDraft({
                  ...draft,
                  platforms: event.target.value
                    .split(",")
                    .map((platform) => platform.trim())
                    .filter(Boolean),
                });
              }}
              placeholder="snes, gba"
            />
          </Field>
        </div>
        <Field label={t("Command template")} hint={t("Must contain %ROM%. Example: retroarch -L snes9x_libretro.so %ROM%")}>
          <Input
            value={draft.command}
            spellCheck={false}
            className="font-mono text-[12px]"
            onChange={(event) => setDraft({ ...draft, command: event.target.value })}
          />
        </Field>
        <div className="mt-2 flex flex-wrap items-center gap-2">
          <Button size="sm" variant="default" onClick={() => void validate()}>
            {t("Validate command")}
          </Button>
          <Button
            size="sm"
            variant="primary"
            icon={<Plus className="size-3.5" />}
            disabled={!draft.name.trim() || !draft.command.includes("%ROM%")}
            onClick={() => {
              setPlatforms("");
              void save({ ...draft, id: draft.id || slug(draft.name) });
            }}
          >
            {t("Save emulator")}
          </Button>
          {validation && (
            <span className={validation.valid ? "text-[11.5px] text-ok" : "text-[11.5px] text-danger"}>
              {validation.valid ? t("Command looks good") : validation.error}
            </span>
          )}
        </div>
      </div>

      <div>
        <p className="font-mono text-[10px] uppercase tracking-[0.1em] text-ink-3">{t("Platform defaults")}</p>
        <div className="mt-2 grid gap-2 sm:grid-cols-2">
          {data.platforms.map((platform) => {
            const candidates = data.registry.emulators.filter((emulator) => emulator.enabled && emulator.platforms.includes(platform.id));
            return (
              <div key={platform.id} className="flex items-center justify-between gap-3 border-b border-line/60 py-1.5">
                <span className="text-[12px] text-ink-2">{platform.label}</span>
                <div className="w-[190px]">
                  <Select
                    label={t("Default emulator for {platform}", { platform: platform.label })}
                    value={data.registry.defaults[platform.id] ?? ""}
                    onChange={(value) => void setDefault(platform.id, value)}
                    options={[{ value: "", label: t("Automatic") }, ...candidates.map((emulator) => ({ value: emulator.id, label: emulator.name }))]}
                  />
                </div>
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

function slug(value: string): string {
  return value
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 48);
}
