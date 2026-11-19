import { describe, expect, it } from "vitest";
import type { AudioVolumeState } from "@/lib/types";
import {
  VOLUME_OSD_MS,
  clampVolumePercent,
  steppedVolumePercent,
  volumeKeyAction,
  volumeOsdFor,
  volumeOsdLabel,
  volumeOsdVisible,
} from "./volume-osd";

/**
 * #289: the console draws its own volume OSD. These tests pin the state the
 * component renders: the percentage source, the clamp and the auto-hide.
 */

const state = (patch: Partial<AudioVolumeState> = {}): AudioVolumeState => ({
  sink: "alsa_output.hdmi",
  label: "HDMI / DisplayPort 1",
  percent: 50,
  muted: false,
  available: true,
  pactl: true,
  ...patch,
});

describe("console volume OSD", () => {
  it("mirrors the state the sound server reported", () => {
    const osd = volumeOsdFor(state(), 1_000);
    expect(osd.sink).toBe("alsa_output.hdmi");
    expect(osd.label).toBe("HDMI / DisplayPort 1");
    expect(osd.percent).toBe(50);
    expect(osd.muted).toBe(false);
    expect(volumeOsdLabel(osd)).toBe("50%");
  });

  it("clamps a bad percentage instead of drawing outside the bar", () => {
    expect(clampVolumePercent(120)).toBe(100);
    expect(clampVolumePercent(-4)).toBe(0);
    expect(clampVolumePercent(Number.NaN)).toBe(0);
    expect(clampVolumePercent(54.6)).toBe(55);
  });

  it("hides itself on the idle deadline", () => {
    const osd = volumeOsdFor(state(), 10_000);
    expect(volumeOsdVisible(osd, 10_000)).toBe(true);
    expect(volumeOsdVisible(osd, 10_000 + VOLUME_OSD_MS - 1)).toBe(true);
    expect(volumeOsdVisible(osd, 10_000 + VOLUME_OSD_MS)).toBe(false);
    expect(volumeOsdVisible(null, 10_000)).toBe(false);
  });

  it("steps by five points between the two ends", () => {
    expect(steppedVolumePercent(50, 1)).toBe(55);
    expect(steppedVolumePercent(50, -1)).toBe(45);
    expect(steppedVolumePercent(98, 1)).toBe(100);
    expect(steppedVolumePercent(3, -1)).toBe(0);
  });

  it("recognises the media keys of every desktop spelling", () => {
    expect(volumeKeyAction("AudioVolumeUp")).toBe("up");
    expect(volumeKeyAction("VolumeDown")).toBe("down");
    expect(volumeKeyAction("XF86AudioRaiseVolume")).toBe("up");
    expect(volumeKeyAction("xf86audiolowervolume")).toBe("down");
    expect(volumeKeyAction("AudioVolumeMute")).toBe("toggleMute");
    expect(volumeKeyAction("Enter")).toBeNull();
  });
});
