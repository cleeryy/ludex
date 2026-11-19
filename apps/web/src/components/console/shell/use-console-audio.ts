import { useCallback, useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { isNative, native } from "@/lib/native";
import { t } from "@/lib/i18n";
import { qk } from "@/lib/query-keys";
import type { AudioSinks, AudioVolumeState } from "@/lib/types";
import { volumeOsdFor, type VolumeOsd, VOLUME_OSD_MS } from "@/lib/volume-osd";
import { useConsoleNotice } from "@/components/console/surfaces/console-notice";

/**
 * The console's audio model (#289).
 *
 * Owns the global output choice and the volume OSD. Every change goes through
 * `pactl` on the desktop: the OSD is Ludex's own surface, never the desktop's,
 * and it reads the state back instead of guessing an increment, so a mute or a
 * device switch cannot drift from the sound server.
 */

export interface ConsoleAudioDevice {
  /** Empty means the sound server's own default. */
  name: string;
  label: string;
}

export interface ConsoleAudioModel {
  supported: boolean;
  devices: ConsoleAudioDevice[];
  /** The global override, or an empty string. */
  selected: string;
  selectedLabel: string;
  volume: number;
  muted: boolean;
  label: string;
  osd: VolumeOsd | null;
  busy: boolean;
  step: (delta: -1 | 1) => void;
  toggleMute: () => void;
  cycleDevice: (delta: -1 | 1) => void;
  test: () => void;
  refresh: () => void;
}

function profileLabel(overview: AudioSinks | undefined): string {
  if (!overview) return t("System default");
  const sink = overview.globalDefault
    ? overview.sinks.find((entry) => entry.name === overview.globalDefault)
    : overview.sinks.find((entry) => entry.name === overview.defaultSink);
  return sink?.description ?? overview.globalDefault ?? t("System default");
}

export function useConsoleAudio(): ConsoleAudioModel {
  const queryClient = useQueryClient();
  const { notify } = useConsoleNotice();
  const [state, setState] = useState<AudioVolumeState | null>(null);
  const [osd, setOsd] = useState<VolumeOsd | null>(null);
  const [busy, setBusy] = useState(false);
  const timer = useRef<number | undefined>(undefined);

  const overview = useQuery({
    queryKey: qk.audioSinks,
    queryFn: () => native.audioSinks(),
    enabled: isNative(),
    staleTime: 15_000,
    retry: false,
  }).data;

  const show = useCallback((next: AudioVolumeState) => {
    setState(next);
    setOsd(volumeOsdFor(next, Date.now()));
    if (timer.current !== undefined) window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setOsd(null), VOLUME_OSD_MS);
  }, []);

  const refresh = useCallback(() => {
    if (!isNative()) return;
    void native
      .audioVolumeState()
      .then((next) => setState(next))
      .catch(() => undefined);
  }, []);

  useEffect(() => {
    refresh();
    return () => {
      if (timer.current !== undefined) window.clearTimeout(timer.current);
    };
  }, [refresh]);

  // A device switch changes the resolved sink; read its volume again.
  useEffect(() => {
    if (overview) refresh();
  }, [overview?.globalDefault, overview?.defaultSink, refresh, overview]);

  // #289: a game fell back to another output. The console has no toast
  // viewport, so the notice surface speaks once, in the Cabinet's language.
  useEffect(() => {
    if (!isNative()) return;
    let disposed = false;
    let off: (() => void) | undefined;
    void native
      .onAudioNotice(({ kind, used }) => {
        if (kind === "noSink") {
          notify({
            title: t("No audio output is available"),
            description: t("Connect an output and the next launch can be routed."),
            tone: "danger",
          });
          return;
        }
        notify({
          title:
            kind === "defaultSinkMissing"
              ? t("The default audio device is not connected")
              : t("The game's audio device is not connected"),
          description: used
            ? t("Falling back to {device}.", { device: used })
            : t("Falling back to the system default."),
          tone: "danger",
        });
      })
      .then((unlisten) => {
        if (disposed) unlisten();
        else off = unlisten;
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      off?.();
    };
  }, [notify]);

  const step = useCallback(
    (delta: -1 | 1) => {
      if (!isNative()) return;
      void native
        .audioVolumeStep(delta > 0 ? "up" : "down")
        .then(show)
        .catch((cause: Error) =>
          notify({ title: t("Could not change the volume"), description: cause.message, tone: "danger" }),
        );
    },
    [notify, show],
  );

  const toggleMute = useCallback(() => {
    if (!isNative()) return;
    void native
      .audioVolumeStep("toggleMute")
      .then(show)
      .catch((cause: Error) =>
        notify({ title: t("Could not change the volume"), description: cause.message, tone: "danger" }),
      );
  }, [notify, show]);

  const cycleDevice = useCallback(
    (delta: -1 | 1) => {
      if (!isNative() || !overview) return;
      const names = ["", ...overview.sinks.map((sink) => sink.name)];
      const current = Math.max(0, names.indexOf(overview.globalDefault));
      const next = names[(current + delta + names.length) % names.length] ?? "";
      setBusy(true);
      void native
        .setAudioDefaultSink(next)
        .then(() => queryClient.invalidateQueries({ queryKey: qk.audioSinks }))
        .then(() => refresh())
        .catch((cause: Error) =>
          notify({ title: t("Could not save the default output"), description: cause.message, tone: "danger" }),
        )
        .finally(() => setBusy(false));
    },
    [notify, overview, queryClient, refresh],
  );

  const test = useCallback(() => {
    if (!isNative()) return;
    const target = state?.sink || overview?.globalDefault || overview?.defaultSink || "";
    if (!target) {
      notify({ title: t("No audio device is available"), description: t("Connect an output and try again."), tone: "danger" });
      return;
    }
    void native
      .audioTestSink(target)
      .then((player) =>
        notify({
          title: t("Test tone playing"),
          description: t("{device} through {player}.", { device: state?.label || target, player }),
          durationMs: 3_000,
        }),
      )
      .catch((cause: Error) =>
        notify({ title: t("Could not play the test tone"), description: cause.message, tone: "danger" }),
      );
  }, [notify, overview, state]);

  const selected = overview?.globalDefault ?? "";
  const selectedLabel = profileLabel(overview);

  return {
    supported: overview?.pactl ?? false,
    devices: [
      { name: "", label: t("System default") },
      ...(overview?.sinks ?? []).map((sink) => ({ name: sink.name, label: sink.description })),
    ],
    selected,
    selectedLabel,
    volume: state?.percent ?? 0,
    muted: state?.muted ?? false,
    label: state?.label ?? selectedLabel,
    osd,
    busy,
    step,
    toggleMute,
    cycleDevice,
    test,
    refresh,
  };
}
