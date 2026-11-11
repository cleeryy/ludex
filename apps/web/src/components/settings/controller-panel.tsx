import { useEffect, useMemo, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Download, Gamepad2, Plus, RotateCcw, Trash2, Upload } from "lucide-react";
import { api } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { cn } from "@/lib/cn";
import {
  CONTROLLER_ACTIONS,
  CONTROLLER_PRESETS,
  inputLabel,
  parseControllerProfile,
  presetProfile,
  serialiseControllerProfile,
  validateControllerProfile,
} from "@/lib/controller-profiles";
import type { ControllerAction, ControllerProfile } from "@/lib/types";
import { useToast } from "@/state/toasts";
import { Badge, Button, Field, Input, Switch } from "@/components/ui/controls";
import { Select } from "@/components/ui/select";
import { t } from "@/lib/i18n";

/**
 * Controller profile editor (#51). Renders the account-wide profile in
 * Settings and a per-game override on the game page. The live panel listens to
 * the first connected pad so a player can find the button they want to remap
 * without guessing indices.
 */
export function ControllerPanel({ gameId = null, gameTitle }: { gameId?: number | null; gameTitle?: string }) {
  const queryClient = useQueryClient();
  const { push } = useToast();
  const profiles = useQuery({ queryKey: ["controller-profiles"], queryFn: () => api.controllerProfiles() });
  const stored = gameId === null ? profiles.data?.global?.profile : profiles.data?.games.find((entry) => entry.gameId === gameId)?.profile;
  const [draft, setDraft] = useState<ControllerProfile>(() => presetProfile("xbox"));
  const [saving, setSaving] = useState(false);
  const [listening, setListening] = useState(false);
  const fileInput = useRef<HTMLInputElement>(null);
  const [pad, setPad] = useState<{ id: string | null; buttons: number[]; axes: number[] }>({ id: null, buttons: [], axes: [] });

  useEffect(() => {
    if (stored) setDraft(stored);
  }, [stored]);

  useEffect(() => {
    if (typeof navigator === "undefined" || typeof navigator.getGamepads !== "function") return;
    let frame = 0;
    let last = 0;
    const loop = (now: number) => {
      if (now - last > 65) {
        last = now;
        const pads = Array.from(navigator.getGamepads?.() ?? []);
        const active = pads.find((entry) => entry?.connected);
        if (active) {
          setPad({
            id: active.id,
            buttons: Array.from(active.buttons).map((button, index) => (button.value > 0.15 ? index : -1)).filter((index) => index >= 0),
            axes: Array.from(active.axes).map((value) => Math.round(value * 100) / 100),
          });
        } else {
          setPad({ id: null, buttons: [], axes: [] });
        }
      }
      frame = requestAnimationFrame(loop);
    };
    frame = requestAnimationFrame(loop);
    return () => cancelAnimationFrame(frame);
  }, []);

  useEffect(() => {
    if (!listening) return;
    const pressed = pad.buttons[0];
    if (pressed === undefined) return;
    const input = `button${pressed}`;
    setDraft((current) => ({
      ...current,
      mappings: current.mappings[input] ? current.mappings : { ...current.mappings, [input]: "confirm" },
    }));
    setListening(false);
  }, [listening, pad.buttons]);

  const error = useMemo(() => validateControllerProfile(draft), [draft]);
  const update = (patch: Partial<ControllerProfile>) => setDraft((current) => ({ ...current, ...patch }));
  const setMapping = (input: string, action: ControllerAction | "") =>
    setDraft((current) => {
      const mappings = { ...current.mappings };
      if (action) mappings[input] = action;
      else delete mappings[input];
      return { ...current, mappings };
    });

  const save = async () => {
    if (error) return;
    setSaving(true);
    try {
      await api.saveControllerProfile(gameId, draft);
      await queryClient.invalidateQueries({ queryKey: ["controller-profiles"] });
      push({ title: t("Controller profile saved"), tone: "success" });
    } catch (cause) {
      push({ title: t("Could not save the controller profile"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setSaving(false);
    }
  };

  const remove = async () => {
    try {
      await api.deleteControllerProfile(gameId ?? 0);
      await queryClient.invalidateQueries({ queryKey: ["controller-profiles"] });
      setDraft(presetProfile("xbox"));
      push({ title: t("Controller profile reset"), tone: "success" });
    } catch (cause) {
      push({ title: t("Could not reset the controller profile"), description: errorMessage(cause), tone: "danger" });
    }
  };

  const exportProfile = () => {
    const blob = new Blob([serialiseControllerProfile(draft)], { type: "application/json" });
    const url = URL.createObjectURL(blob);
    const link = document.createElement("a");
    link.href = url;
    link.download = `ludex-controller-${draft.name.toLowerCase().replace(/[^a-z0-9]+/g, "-")}.json`;
    link.click();
    URL.revokeObjectURL(url);
  };

  const importProfile = async (file: File | undefined) => {
    if (!file) return;
    try {
      const profile = parseControllerProfile(await file.text());
      setDraft(profile);
      push({ title: t("Controller profile imported"), tone: "success" });
    } catch (cause) {
      push({ title: t("Invalid controller profile"), description: errorMessage(cause), tone: "danger" });
    } finally {
      if (fileInput.current) fileInput.current.value = "";
    }
  };

  const mappings = Object.entries(draft.mappings).sort(([left], [right]) => left.localeCompare(right));

  return (
    <section className="rounded-[3px] border border-line bg-bg-2/40 p-4 sm:p-5">
      <header className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h3 className="flex items-center gap-2 font-mono text-[11px] uppercase tracking-[0.12em] text-ink-2">
            <Gamepad2 className="size-3.5" /> {t("Controller profile")}
          </h3>
          <p className="mt-1 text-[12.5px] text-ink-3">
            {gameId === null
              ? t("Applies to every game unless that game has its own profile.")
              : t("Overrides your account profile while playing {title}.", { title: gameTitle ?? t("this game") })}
          </p>
        </div>
        <Badge tone={pad.id ? "signal" : "outline"}>
          {pad.id ? t("Pad detected") : t("No gamepad detected")}
        </Badge>
      </header>

      <div className="mt-4 grid gap-4 sm:grid-cols-2">
        <Field label={t("Name")}>
          <Input value={draft.name} maxLength={64} onChange={(event) => update({ name: event.target.value })} />
        </Field>
        <Field label={t("Start from a preset")}>
          <Select
            label={t("Preset")}
            value=""
            onChange={(value) => {
              if (value) setDraft(presetProfile(value));
            }}
            options={[
              { value: "", label: t("Choose a preset") },
              ...CONTROLLER_PRESETS.map((id) => ({ value: id, label: presetProfile(id).name })),
            ]}
          />
        </Field>
      </div>

      <div className="mt-4 grid gap-4 sm:grid-cols-2">
        <Range
          label={t("Deadzone")}
          value={draft.deadzone}
          min={0}
          max={0.9}
          step={0.01}
          format={(value) => value.toFixed(2)}
          onChange={(deadzone) => update({ deadzone })}
        />
        <Range
          label={t("Sensitivity")}
          value={draft.sensitivity}
          min={0.2}
          max={2.5}
          step={0.05}
          format={(value) => `${value.toFixed(2)}×`}
          onChange={(sensitivity) => update({ sensitivity })}
        />
        <Range
          label={t("Response curve")}
          value={draft.curve}
          min={0.5}
          max={3}
          step={0.05}
          format={(value) => value.toFixed(2)}
          onChange={(curve) => update({ curve })}
        />
        <Range
          label={t("Trigger threshold")}
          value={draft.triggerThreshold}
          min={0}
          max={0.95}
          step={0.01}
          format={(value) => value.toFixed(2)}
          onChange={(triggerThreshold) => update({ triggerThreshold })}
        />
      </div>

      <div className="mt-3 flex flex-wrap gap-x-6 gap-y-2">
        <Switch checked={draft.invertX} onCheckedChange={(invertX) => update({ invertX })} label={t("Invert horizontal stick")} />
        <Switch checked={draft.invertY} onCheckedChange={(invertY) => update({ invertY })} label={t("Invert vertical stick")} />
        <Switch checked={draft.vibration} onCheckedChange={(vibration) => update({ vibration })} label={t("Vibration")} />
      </div>

      <div className="mt-4 rounded-[2px] border border-line/70 bg-bg/40 px-3 py-2 font-mono text-[11px] text-ink-3">
        {pad.id ? (
          <div className="flex flex-wrap items-center gap-x-4 gap-y-1">
            <span className="truncate">{pad.id}</span>
            <span>
              {t("Buttons")}: {pad.buttons.length ? pad.buttons.join(", ") : "—"}
            </span>
            <span>
              {t("Axes")}: {pad.axes.slice(0, 4).join(" · ")}
            </span>
          </div>
        ) : (
          <span>{t("Connect a controller and press a button to see it here.")}</span>
        )}
      </div>

      <div className="mt-4">
        <div className="mb-2 flex items-center justify-between gap-2">
          <h4 className="font-mono text-[10px] uppercase tracking-[0.12em] text-ink-3">{t("Button map")}</h4>
          <Button size="sm" variant="ghost" icon={<Plus className="size-3.5" />} onClick={() => setListening(true)}>
            {listening ? t("Press a button…") : t("Listen for a button")}
          </Button>
        </div>
        <ul className="divide-y divide-line/60 border-y border-line/60">
          {mappings.map(([input, action]) => (
            <li key={input} className="grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto] items-center gap-2 py-1.5">
              <span className="truncate font-mono text-[11.5px] text-ink-2">{inputLabel(input)}</span>
              <Select
                label={inputLabel(input)}
                value={action}
                onChange={(value) => setMapping(input, value as ControllerAction)}
                options={[
                  ...CONTROLLER_ACTIONS.map((value) => ({ value, label: actionLabel(value) })),
                  { value: "", label: t("Unmapped") },
                ]}
              />
              <button
                type="button"
                className="rounded-[2px] p-1.5 text-ink-3 transition-colors hover:text-danger"
                aria-label={t("Remove mapping for {input}", { input: inputLabel(input) })}
                onClick={() => setMapping(input, "")}
              >
                <Trash2 className="size-3.5" />
              </button>
            </li>
          ))}
        </ul>
      </div>

      {error && <p className="mt-3 text-[12px] text-danger">{error}</p>}

      <footer className="mt-4 flex flex-wrap items-center gap-2">
        <Button variant="primary" onClick={save} disabled={Boolean(error) || saving || !profiles.data}>
          {saving ? t("Saving…") : t("Save profile")}
        </Button>
        <Button variant="ghost" icon={<RotateCcw className="size-3.5" />} onClick={remove} disabled={!stored}>
          {t("Reset")}
        </Button>
        <Button variant="ghost" icon={<Download className="size-3.5" />} onClick={exportProfile}>
          {t("Export JSON")}
        </Button>
        <Button variant="ghost" icon={<Upload className="size-3.5" />} onClick={() => fileInput.current?.click()}>
          {t("Import JSON")}
        </Button>
        <input
          ref={fileInput}
          type="file"
          accept="application/json"
          className="sr-only"
          aria-label={t("Import a controller profile")}
          onChange={(event) => void importProfile(event.target.files?.[0])}
        />
      </footer>
    </section>
  );
}

function Range({
  label,
  value,
  min,
  max,
  step,
  format,
  onChange,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  format: (value: number) => string;
  onChange: (value: number) => void;
}) {
  return (
    <label className="flex flex-col gap-1.5 text-[12.5px] text-ink-2">
      <span className="flex items-baseline justify-between">
        {label}
        <span className="font-mono text-[11px] text-ink-3">{format(value)}</span>
      </span>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(event) => onChange(Number(event.target.value))}
        className={cn("h-1.5 w-full accent-[var(--signal)]")}
      />
    </label>
  );
}

function actionLabel(action: ControllerAction): string {
  switch (action) {
    case "left":
      return t("Left");
    case "right":
      return t("Right");
    case "up":
      return t("Up");
    case "down":
      return t("Down");
    case "confirm":
      return t("Confirm");
    case "back":
      return t("Back");
    case "menu":
      return t("Menu");
    case "downloads":
      return t("Downloads");
    case "prevTab":
      return t("Previous tab");
    case "nextTab":
      return t("Next tab");
  }
}
