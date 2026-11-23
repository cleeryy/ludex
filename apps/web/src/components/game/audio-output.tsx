import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Volume2 } from "lucide-react";
import { errorMessage } from "@/lib/http";
import { isNative, native } from "@/lib/native";
import { t } from "@/lib/i18n";
import { qk } from "@/lib/query-keys";
import type { AudioSink } from "@/lib/types";
import { useToast } from "@/state/toasts";
import { Button } from "@/components/ui/controls";
import { Select } from "@/components/ui/select";
import { SectionHeader } from "@/components/page";

/**
 * Per-game audio output (#289).
 *
 * The device list comes from `pactl`; a game routed to a headset keeps that
 * headset while the TV stays on the desktop default. A device that is no longer
 * plugged in is named instead of being silently replaced: the launch falls back
 * to the profile default and the same fallback is reported as a toast.
 *
 * The global override lives here too, because "use this device for every game"
 * is the answer to the common desk setup, and burying it in Settings would make
 * the per-game choice feel like the only one.
 */
export function AudioOutputPanel({ gameId, title }: { gameId: number; title: string }) {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const [busy, setBusy] = useState(false);

  const sinksQuery = useQuery({
    queryKey: qk.audioSinks,
    queryFn: () => native.audioSinks(),
    enabled: isNative(),
    staleTime: 15_000,
    retry: false,
  });
  const optionsQuery = useQuery({
    queryKey: qk.launchOptions(gameId),
    queryFn: () => native.getLaunchOptions(gameId),
    enabled: isNative(),
    retry: false,
  });

  if (!isNative()) return null;

  const overview = sinksQuery.data;
  const saved = optionsQuery.data?.audioSink ?? "";
  const sinks = overview?.sinks ?? [];
  const savedSink = sinks.find((sink) => sink.name === saved) ?? null;
  const fallbackUsed = overview?.globalDefault
    ? sinks.find((sink) => sink.name === overview.globalDefault) ?? null
    : sinks.find((sink) => sink.name === overview?.defaultSink) ?? sinks[0] ?? null;
  const missing = saved !== "" && sinksQuery.data !== undefined && savedSink === null;

  const saveGameSink = async (sink: string) => {
    const current = optionsQuery.data;
    if (!current) return;
    setBusy(true);
    try {
      await native.setLaunchOptions(gameId, { ...current, audioSink: sink });
      await queryClient.invalidateQueries({ queryKey: qk.launchOptions(gameId) });
      push({
        title: t("Audio output saved"),
        description: sink
          ? t("{title} starts on {device}.", { title, device: sinks.find((entry) => entry.name === sink)?.description ?? sink })
          : t("{title} follows the default output.", { title }),
        tone: "success",
      });
    } catch (cause) {
      push({ title: t("Could not save the audio output"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setBusy(false);
    }
  };

  const test = async (sink: AudioSink | null) => {
    const target = sink?.name ?? savedSink?.name ?? fallbackUsed?.name ?? "";
    if (!target) {
      push({ title: t("No audio device is available"), description: t("Connect an output and try again."), tone: "danger" });
      return;
    }
    try {
      const player = await native.audioTestSink(target);
      push({
        title: t("Test tone playing"),
        description: t("{device} through {player}.", { device: sink?.description ?? target, player }),
        tone: "success",
      });
    } catch (cause) {
      push({ title: t("Could not play the test tone"), description: errorMessage(cause), tone: "danger" });
    }
  };

  const setGlobal = async (sink: string) => {
    setBusy(true);
    try {
      await native.setAudioDefaultSink(sink);
      await queryClient.invalidateQueries({ queryKey: qk.audioSinks });
      await queryClient.invalidateQueries({ queryKey: qk.desktopSettings });
      push({
        title: sink ? t("Default audio output saved") : t("Default audio output cleared"),
        description: sink
          ? t("Games without their own device now start on {device}.", {
              device: sinks.find((entry) => entry.name === sink)?.description ?? sink,
            })
          : t("Games without their own device follow the system default again."),
        tone: "success",
      });
    } catch (cause) {
      push({ title: t("Could not save the default output"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setBusy(false);
    }
  };

  const supported = overview?.pactl ?? true;
  const profileLabel = overview?.globalDefault
    ? sinks.find((sink) => sink.name === overview.globalDefault)?.description ?? overview.globalDefault
    : sinks.find((sink) => sink.name === overview?.defaultSink)?.description ?? t("System default");

  return (
    <section aria-labelledby="game-audio-heading" className="flex flex-col gap-3">
      <SectionHeader
        level={3}
        id="game-audio-heading"
        title={t("Audio output")}
        description={t("Send this game to one device while the rest of the desktop stays on another.")}
        actions={
          <Button
            variant="ghost"
            size="sm"
            className="h-8"
            disabled={busy || !supported}
            onClick={() => void test(savedSink)}
          >
            <Volume2 className="size-3.5" aria-hidden="true" />
            {t("Test")}
          </Button>
        }
      />

      {!supported ? (
        <p className="text-meta leading-relaxed text-ink-3">
          {t("Audio routing needs PipeWire or PulseAudio (pactl) on this Linux desktop.")}
        </p>
      ) : (
        <>
          <label className="flex max-w-[46rem] flex-col gap-1.5">
            <span className="text-label text-ink-3">{t("Device for this game")}</span>
            <Select
              label={t("Device for this game")}
              value={saved}
              disabled={busy}
              onChange={(sink) => void saveGameSink(sink)}
              options={[
                {
                  value: "",
                  label: t("Use the default output"),
                  hint: profileLabel,
                },
                ...sinks.map((sink) => ({
                  value: sink.name,
                  label: sink.description,
                  hint: sink.isDefault ? t("Current system default") : sink.name,
                })),
              ]}
            />
          </label>

          {missing ? (
            <p role="status" className="border-l-2 border-warn/60 pl-3 text-meta leading-relaxed text-warn">
              {t("The saved device is not connected. Until it returns, the game starts on {device}.", {
                device: fallbackUsed?.description ?? t("the system default"),
              })}
            </p>
          ) : (
            <p className="text-meta leading-relaxed text-ink-3">
              {saved
                ? t("Only this game is moved; the stream follows the game's process tree.")
                : t("The game follows the default output: {device}.", { device: profileLabel })}
            </p>
          )}

          <div className="flex flex-wrap items-center gap-2">
            <Button
              variant="ghost"
              size="sm"
              className="h-8"
              disabled={busy || !savedSink}
              onClick={() => void setGlobal(savedSink?.name ?? "")}
            >
              {t("Use this device for every game")}
            </Button>
            {overview?.globalDefault ? (
              <Button variant="ghost" size="sm" className="h-8" disabled={busy} onClick={() => void setGlobal("")}>
                {t("Follow the system default")}
              </Button>
            ) : null}
          </div>
          <p className="text-meta leading-relaxed text-ink-3">
            {overview?.globalDefault
              ? t("All games default to {device}. A per-game device still wins.", { device: profileLabel })
              : t("No global override: games without their own device follow the system default.")}
          </p>
        </>
      )}
    </section>
  );
}
