import { describe, expect, it } from "vitest";
import {
  INITIAL_REMOTE_PLAY_WIZARD,
  addressChoices,
  leaseNeedsRefresh,
  minutesUntilExpiry,
  moonlightAddress,
  portSummary,
  remotePlayFailure,
  remotePlayWizard,
  type RemotePlayWizard,
} from "./remote-play";

const status = {
  vpn: { address: null as string | null },
  publicIp: "198.51.100.4",
  lanIp: "192.168.1.20",
  active: { mappings: [] as unknown[] },
};

describe("remote play wizard", () => {
  it("starts on detection and only advances once detection ran", () => {
    expect(INITIAL_REMOTE_PLAY_WIZARD.step).toBe("detect");
    const wizard = remotePlayWizard(INITIAL_REMOTE_PLAY_WIZARD, { type: "detected" });
    expect(wizard).toEqual({ step: "mode", mode: null, error: null });
  });

  it("sends VPN and LAN modes straight to the Moonlight instructions", () => {
    const vpn = remotePlayWizard(
      { step: "mode", mode: null, error: null },
      { type: "choose", mode: "vpn" },
    );
    expect(vpn).toEqual({ step: "test", mode: "vpn", error: null });
    const lan = remotePlayWizard(vpn, { type: "back" });
    expect(lan.step).toBe("mode");
    expect(
      remotePlayWizard(lan, { type: "choose", mode: "lan" }).step,
    ).toBe("test");
  });

  it("routes advanced UPnP mode through consent, mapping and verification", () => {
    let wizard: RemotePlayWizard = { step: "mode", mode: null, error: null };
    wizard = remotePlayWizard(wizard, { type: "choose", mode: "upnp" });
    expect(wizard.step).toBe("consent");
    wizard = remotePlayWizard(wizard, { type: "consent" });
    expect(wizard.step).toBe("map");
    wizard = remotePlayWizard(wizard, { type: "mapped" });
    expect(wizard.step).toBe("verify");
    wizard = remotePlayWizard(wizard, { type: "verified" });
    expect(wizard.step).toBe("test");
    expect(wizard.mode).toBe("upnp");
  });

  it("keeps the step and exposes failures until a retry", () => {
    const failed = remotePlayWizard(
      { step: "map", mode: "upnp", error: null },
      { type: "failed", message: "CGNAT" },
    );
    expect(failed.step).toBe("map");
    expect(failed.error).toBe("CGNAT");
    const retried = remotePlayWizard(failed, { type: "retry" });
    expect(retried.error).toBeNull();
    expect(retried.step).toBe("map");
  });

  it("walks back one meaningful step at a time", () => {
    let wizard: RemotePlayWizard = { step: "test", mode: "upnp", error: null };
    wizard = remotePlayWizard(wizard, { type: "back" });
    expect(wizard.step).toBe("verify");
    wizard = remotePlayWizard(wizard, { type: "back" });
    expect(wizard.step).toBe("consent");
    wizard = remotePlayWizard(wizard, { type: "back" });
    expect(wizard.step).toBe("mode");
    wizard = remotePlayWizard(wizard, { type: "back" });
    expect(wizard.step).toBe("detect");
    expect(wizard.mode).toBeNull();
  });

  it("resumes an active session and resets cleanly", () => {
    const resumed = remotePlayWizard(INITIAL_REMOTE_PLAY_WIZARD, { type: "resumed" });
    expect(resumed.step).toBe("test");
    expect(remotePlayWizard(resumed, { type: "reset" })).toEqual(
      INITIAL_REMOTE_PLAY_WIZARD,
    );
  });
});

describe("remote play helpers", () => {
  it("summarises the exact ports shown for consent", () => {
    expect(portSummary({ tcp: [47984, 47989, 48010], udp: [47998, 48000] })).toBe(
      "TCP 47984, 47989, 48010 · UDP 47998, 48000",
    );
    expect(portSummary({ tcp: [], udp: [48010] })).toBe("UDP 48010");
  });

  it("prefers the VPN address, then the public address only when mapped", () => {
    expect(moonlightAddress(status)).toBe("192.168.1.20");
    expect(
      moonlightAddress({ ...status, active: { mappings: [{}] } }),
    ).toBe("198.51.100.4");
    expect(
      moonlightAddress({ ...status, vpn: { address: "100.101.102.103" } }),
    ).toBe("100.101.102.103");
  });

  it("uses the advertised address before the raw LAN fallback", () => {
    expect(
      moonlightAddress({ ...status, advertised: { address: "100.64.0.5" } }),
    ).toBe("100.64.0.5");
    expect(
      moonlightAddress({ ...status, advertised: { address: null } }),
    ).toBe("192.168.1.20");
  });

  it("offers usable addresses only, advertised first", () => {
    const choices = addressChoices({
      advertised: { address: "100.101.102.103" },
      addresses: [
        { address: "172.17.0.1", kind: "virtual" },
        { address: "192.168.1.8", kind: "lan" },
        { address: "100.101.102.103", kind: "vpn" },
        { address: "fe80::1", kind: "link_local" },
        { address: "2001:db8::8", kind: "ipv6" },
      ],
    });
    expect(choices.map((entry) => entry.address)).toEqual([
      "100.101.102.103",
      "192.168.1.8",
      "2001:db8::8",
    ]);
    expect(
      addressChoices({
        advertised: { address: null },
        addresses: [{ address: "100.64.3.7", kind: "cgnat" }],
      }),
    ).toEqual([]);
  });

  it("refreshes a lease before half of it elapsed", () => {
    const session = { mappings: [{ expiresAt: 4_600, leaseSecs: 3_600 }] };
    expect(leaseNeedsRefresh(session, 2_000)).toBe(false);
    expect(leaseNeedsRefresh(session, 3_000)).toBe(true);
    expect(minutesUntilExpiry(session, 2_200)).toBe(40);
    expect(minutesUntilExpiry({ mappings: [] }, 0)).toBeNull();
  });

  it("reads the structured Tauri error and falls back to a string", () => {
    expect(
      remotePlayFailure({ code: "cgnat", message: "Carrier NAT" }),
    ).toEqual({ code: "cgnat", message: "Carrier NAT" });
    expect(remotePlayFailure("boom")).toEqual({ code: "unknown", message: "boom" });
    expect(remotePlayFailure(null)).toEqual({ code: "unknown", message: "" });
  });
});
