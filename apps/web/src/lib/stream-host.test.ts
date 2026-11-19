import { describe, expect, it } from "vitest";
import {
  editSummary,
  packetSizeForMtu,
  portRange,
  portSummary,
  selfTestCopy,
  statusLabel,
  statusTone,
  streamCheckCopy,
} from "./stream-host";
import type { FirewallRule } from "./types";

const RULES: FirewallRule[] = [
  { protocol: "tcp", first: 47984, last: 47990, purpose: "pairing" },
  { protocol: "tcp", first: 48010, last: 48010, purpose: "rtsp" },
  { protocol: "udp", first: 47998, last: 48010, purpose: "media" },
];

const CODES = [
  "sunshine-missing",
  "sunshine-disabled",
  "capture-unavailable",
  "no-output",
  "display-blanked",
  "wrong-output",
  "encoder-unavailable",
  "mtu-low",
];

describe("streaming host copy", () => {
  it("gives every self-test code its own title and fix", () => {
    const titles = new Set<string>();
    for (const code of CODES) {
      const copy = selfTestCopy(code);
      expect(copy, code).not.toBeNull();
      expect(copy?.title.length).toBeGreaterThan(0);
      expect(copy?.fix.length).toBeGreaterThan(0);
      expect(titles.has(copy!.title), `${code} duplicates ${copy!.title}`).toBe(false);
      titles.add(copy!.title);
    }
    expect(selfTestCopy("not-a-code")).toBeNull();
  });

  it("labels the five detection rows", () => {
    expect(streamCheckCopy("sunshine")).toBe("Sunshine binary");
    expect(streamCheckCopy("service")).toBe("Sunshine service");
    expect(streamCheckCopy("capture")).toBe("Screen capture");
    expect(streamCheckCopy("encoder")).toBe("Video encoder");
    expect(streamCheckCopy("output")).toBe("Display output");
    expect(streamCheckCopy("other")).toBe("Host check");
  });

  it("maps check statuses to badge tones and labels", () => {
    expect(statusTone("ok")).toBe("ok");
    expect(statusTone("warning")).toBe("signal");
    expect(statusTone("broken")).toBe("danger");
    expect(statusTone("missing")).toBe("danger");
    expect(statusTone("unknown")).toBe("outline");
    expect(statusLabel("ok")).toBe("ok");
    expect(statusLabel("unknown")).toBe("unknown");
  });

  it("shows exact ports, never a range for a single port", () => {
    expect(portRange({ first: 48010, last: 48010 })).toBe("48010");
    expect(portRange({ first: 47984, last: 47990 })).toBe("47984-47990");
    expect(portSummary(RULES)).toBe("TCP 47984-47990, 48010 · UDP 47998-48010");
    expect(portSummary([])).toBe("");
  });

  it("previews the same MTU-to-packet-size table as the planner", () => {
    const table: Array<[number, number]> = [
      [0, 1024],
      [576, 1024],
      [1100, 1024],
      [1200, 1120],
      [1220, 1140],
      [1280, 1200],
      [1300, 1220],
      [1400, 1320],
      [1420, 1340],
      [1500, 1420],
      [9000, 1420],
    ];
    for (const [mtu, packet] of table) {
      expect(packetSizeForMtu(mtu), `MTU ${mtu}`).toBe(packet);
    }
  });

  it("renders an edit as an old-to-new line", () => {
    expect(editSummary({ key: "output_name", value: "DP-1", previous: "HDMI-A-1" })).toBe(
      "output_name: HDMI-A-1 → DP-1",
    );
    expect(editSummary({ key: "packet_size", value: "1200", previous: null })).toBe(
      "packet_size = 1200",
    );
  });
});
