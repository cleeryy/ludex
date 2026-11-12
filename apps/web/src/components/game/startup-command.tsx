import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { api } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { isNative, native } from "@/lib/native";
import type { GameDetail, InstalledGame } from "@/lib/types";
import { useToast } from "@/state/toasts";
import { Button, Input } from "@/components/ui/controls";
import { SectionHeader } from "@/components/page";
import { ExecutableField } from "@/components/game/executable-picker";
import { cn } from "@/lib/cn";
import { t } from "@/lib/i18n";

/**
 * The executable/parameters the desktop app uses to start the game (#143).
 * Editor-only, and always expanded: it now has an explicit destination
 * (`?tab=launch`) instead of a collapsed block at the bottom of the page.
 */
export function StartupCommandPanel({
  game,
  installed,
  level = 3,
}: {
  game: GameDetail;
  installed?: InstalledGame;
  level?: 2 | 3;
}) {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const [executable, setExecutable] = useState(game.launch.executable ?? "");
  const [parameters, setParameters] = useState(game.launch.parameters ?? "");
  const [installer, setInstaller] = useState(game.launch.installerExecutable ?? "");
  const [uninstaller, setUninstaller] = useState(game.launch.uninstallerExecutable ?? "");
  const [busy, setBusy] = useState(false);

  // The picker needs a folder on this device; a game installed elsewhere (or
  // not installed yet) keeps the manual fields (#143).
  const canPick = Boolean(installed && isNative());

  const save = async () => {
    setBusy(true);
    try {
      let nextExecutable = executable.trim();
      let nextInstaller = installer.trim();
      let nextUninstaller = uninstaller.trim();
      if (canPick) {
        // Validate against the install folder before writing the server
        // config: a path that escapes it is refused here, not at launch.
        // An imported row without a folder on this device has nothing to
        // validate against, so its manual values are kept as typed.
        const listing = await native.listGameExecutables(game.id).catch((cause: unknown) => {
          const message = errorMessage(cause);
          if (message.includes("install folder is missing") || message.includes("has no folder")) return null;
          throw cause;
        });
        if (listing) {
          if (nextExecutable) nextExecutable = await native.checkGameExecutable(game.id, nextExecutable);
          if (nextInstaller) nextInstaller = await native.checkGameExecutable(game.id, nextInstaller);
          if (nextUninstaller) nextUninstaller = await native.checkGameExecutable(game.id, nextUninstaller);
          setExecutable(nextExecutable);
          setInstaller(nextInstaller);
          setUninstaller(nextUninstaller);
        }
      }
      await api.patchGame(game.id, {
        launch: {
          executable: nextExecutable || null,
          parameters: parameters || null,
          installerExecutable: nextInstaller || null,
          uninstallerExecutable: nextUninstaller || null,
        },
      });
      // The desktop app launches `installed.executable` when one is recorded,
      // so the picker must update that row too; otherwise the saved choice
      // would never be used (#143).
      let localExecutableSaved = true;
      if (canPick && nextExecutable) {
        try {
          await native.setExecutable(game.id, nextExecutable);
          await queryClient.invalidateQueries({ queryKey: ["installed"] });
        } catch (cause) {
          localExecutableSaved = false;
          push({
            title: t("Startup command saved"),
            description: errorMessage(cause),
            tone: "default",
          });
        }
      }
      if (localExecutableSaved) push({ title: t("Startup command saved"), tone: "success" });
      await queryClient.invalidateQueries({ queryKey: ["game", game.id] });
    } catch (cause) {
      push({ title: t("Could not save"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="min-w-0" aria-labelledby="startup-command-heading">
      <SectionHeader
        id="startup-command-heading"
        level={level}
        title={t("Startup command")}
        description={t("Used by the desktop app when it starts the game")}
      />
      <div className="grid gap-4 border-t border-line pt-4 sm:grid-cols-2">
        {canPick ? (
          <div className="sm:col-span-2">
            <ExecutableField
              gameId={game.id}
              kind="launch"
              label={t("Executable")}
              value={executable}
              onChange={setExecutable}
              autoSelectRecommended
              hint={t("Programs found in the install folder. The detected one is marked.")}
            />
          </div>
        ) : (
          <label className="flex flex-col gap-1.5">
            <span className="text-[12px] text-ink-2">{t("Executable")}</span>
            <Input value={executable} onChange={(event) => setExecutable(event.target.value)} placeholder="Game.exe" className="h-10 font-mono text-[12px]" />
          </label>
        )}
        <label className={cn("flex flex-col gap-1.5", canPick && "sm:col-span-2")}>
          <span className="text-[12px] text-ink-2">{t("Parameters")}</span>
          <Input value={parameters} onChange={(event) => setParameters(event.target.value)} placeholder="-windowed" className="h-10 font-mono text-[12px]" />
        </label>
        {canPick ? (
          <>
            <div className="sm:col-span-2">
              <ExecutableField
                gameId={game.id}
                kind="installer"
                label={t("Installer")}
                value={installer}
                onChange={setInstaller}
                hint={t("setup, install and gog programs found in the install folder.")}
              />
            </div>
            <div className="sm:col-span-2">
              <ExecutableField
                gameId={game.id}
                kind="uninstaller"
                label={t("Uninstaller")}
                value={uninstaller}
                onChange={setUninstaller}
                hint={t("unins and uninstall programs found in the install folder.")}
              />
            </div>
          </>
        ) : (
          <>
            <label className="flex flex-col gap-1.5">
              <span className="text-[12px] text-ink-2">{t("Installer")}</span>
              <Input value={installer} onChange={(event) => setInstaller(event.target.value)} placeholder="setup.exe" className="h-10 font-mono text-[12px]" />
            </label>
            <label className="flex flex-col gap-1.5">
              <span className="text-[12px] text-ink-2">{t("Uninstaller")}</span>
              <Input value={uninstaller} onChange={(event) => setUninstaller(event.target.value)} placeholder="unins000.exe" className="h-10 font-mono text-[12px]" />
            </label>
          </>
        )}
        <div className="sm:col-span-2">
          <Button variant="default" loading={busy} onClick={() => void save()}>
            {t("Save startup command")}
          </Button>
        </div>
      </div>
    </section>
  );
}
