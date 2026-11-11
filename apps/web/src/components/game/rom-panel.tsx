import { useMemo, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Gamepad2, Play } from "lucide-react";
import { api } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { isNative, native } from "@/lib/native";
import type { GameDetail } from "@/lib/types";
import { useIsEditor } from "@/state/auth";
import { useToast } from "@/state/toasts";
import { Badge, Button } from "@/components/ui/controls";
import { Select } from "@/components/ui/select";
import { t } from "@/lib/i18n";

/**
 * ROM and emulator panel (#52). Shows the detected platform, lets an editor
 * pick a per-game emulator override and, in the desktop app, launches the ROM
 * through the user's emulator registry with a recorded play session.
 *
 * The browser build never tries to run a local binary: the ROM lives on the
 * server, and the desktop app is the only place where spawning is meaningful.
 */
export function RomPanel({ game }: { game: GameDetail }) {
  const isEditor = useIsEditor();
  const { push } = useToast();
  const queryClient = useQueryClient();
  const [launching, setLaunching] = useState(false);
  const platforms = useQuery({ queryKey: ["platforms"], queryFn: () => api.listPlatforms() });
  const emulators = useQuery({ queryKey: ["emulators"], queryFn: () => api.listEmulators() });
  const platform = useMemo(
    () => platforms.data?.find((entry) => entry.id === game.platform),
    [platforms.data, game.platform],
  );
  if (game.kind !== "rom") return null;

  const launch = async () => {
    if (!isNative() || !game.platform) return;
    setLaunching(true);
    try {
      const resolved = await api.launchCommand(game.id);
      const session = await native.launchRomGame({
        gameId: game.id,
        platform: resolved.platform,
        romPath: resolved.romPath,
      });
      push({
        title: t("Launching {title}", { title: game.title }),
        description: t("Started with {emulator}. Playtime is recorded like any other game.", {
          emulator: session.emulatorName,
        }),
        tone: "success",
      });
      await queryClient.invalidateQueries({ queryKey: ["game", game.id] });
    } catch (cause) {
      push({ title: t("Could not launch the ROM"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setLaunching(false);
    }
  };

  const setEmulator = async (value: string) => {
    const emulatorId = value ? Number(value) : null;
    try {
      await api.setGameEmulator(game.id, emulatorId);
      await queryClient.invalidateQueries({ queryKey: ["game", game.id] });
      push({ title: t("Emulator updated"), tone: "success" });
    } catch (cause) {
      push({ title: t("Could not update the emulator"), description: errorMessage(cause), tone: "danger" });
    }
  };

  return (
    <section className="mt-12" aria-labelledby="rom-heading">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h2 id="rom-heading" className="flex items-center gap-2 font-mono text-[10px] uppercase tracking-[0.12em] text-ink-3">
            <Gamepad2 className="size-3.5" /> {t("ROM and emulator")}
          </h2>
          <p className="mt-1 text-[12.5px] text-ink-2">
            {platform?.label ?? game.platform ?? t("Unrecognised platform")}
            {platform?.retroarchCore && (
              <span className="ml-2 font-mono text-[10.5px] text-ink-3">core {platform.retroarchCore}</span>
            )}
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <Badge tone="outline">{t("ROM")}</Badge>
          {isNative() && game.platform && (
            <Button variant="primary" icon={<Play className="size-3.5" />} loading={launching} onClick={() => void launch()}>
              {t("Launch on this device")}
            </Button>
          )}
        </div>
      </div>

      <div className="mt-3 border-t border-line pt-3">
        {isEditor ? (
          <div className="max-w-[360px]">
            <Select
              label={t("Emulator")}
              value={game.emulatorId ? String(game.emulatorId) : ""}
              onChange={(value) => void setEmulator(value)}
              options={[
                { value: "", label: t("Platform default") },
                ...(emulators.data ?? [])
                  .filter((emulator) => !game.platform || emulator.platforms.includes(game.platform))
                  .map((emulator) => ({
                    value: String(emulator.id),
                    label: emulator.name,
                    hint: emulator.enabled ? undefined : t("Disabled"),
                  })),
              ]}
            />
            <p className="mt-2 text-[11.5px] leading-relaxed text-ink-3">
              {t("Emulator commands are configured by an administrator. A per-game pick overrides the platform default.")}
            </p>
          </div>
        ) : (
          <p className="text-[11.5px] leading-relaxed text-ink-3">
            {isNative()
              ? t("Ludex resolves the emulator command locally; no emulator binary is bundled.")
              : t("ROM files stay on the server. Open this page in the Ludex desktop app to launch the game.")}
          </p>
        )}
      </div>
    </section>
  );
}
