import { describe, expect, it } from "vitest";
import {
  canToggle,
  entryAction,
  managerLabel,
  needsSeatAttention,
  planCounts,
  plannedFileKind,
  toggleExplanation,
} from "./console-session";
import type {
  ConsoleSessionPlan,
  ConsoleSessionPlannedFile,
  ConsoleSessionStatus,
} from "./types";

const file = (
  path: string,
  action: "write" | "remove",
  existed = true,
): ConsoleSessionPlannedFile => ({
  path,
  action,
  contents: "",
  existed,
  backup: action === "write" && existed ? `/backups/${path}` : null,
  mode: 0o644,
  description: path,
});

const plan = (enable: boolean, files: ConsoleSessionPlannedFile[]): ConsoleSessionPlan => ({
  enable,
  manager: { id: "sddm", name: "SDDM", configPaths: [] },
  user: "player",
  session: "ludex-console",
  files,
  stateFile: "/var/lib/ludex/console-session/state.json",
  commands: [],
  warnings: [],
  message: "plan",
});

const status = (patch: Partial<ConsoleSessionStatus> = {}): ConsoleSessionStatus => ({
  platform: "linux",
  supported: true,
  manager: { id: "sddm", name: "SDDM", configPaths: ["/etc/sddm.conf.d/90.conf"] },
  managerReason: null,
  user: "player",
  session: "ludex-console",
  entryPath: "/usr/share/wayland-sessions/ludex-console.desktop",
  entryState: "missing",
  entryInstalled: false,
  enabled: false,
  managedFiles: [],
  seat: { sessionType: "wayland", active: true, compatible: true, note: "ok" },
  pkexecAvailable: true,
  message: "ready",
  ...patch,
});

describe("console session plan counts", () => {
  it("counts writes and backups on enable", () => {
    const counts = planCounts(
      plan(true, [
        file("/usr/share/wayland-sessions/ludex-console.desktop", "write"),
        file("/etc/sddm.conf.d/90-ludex-console.conf", "write", false),
      ]),
    );
    expect(counts).toEqual({ writes: 2, restores: 0, removes: 0, backups: 1 });
  });

  it("counts restores and removals on disable", () => {
    const counts = planCounts(
      plan(false, [
        file("/etc/sddm.conf.d/90-ludex-console.conf", "write"),
        file("/usr/share/wayland-sessions/ludex-console.desktop", "remove"),
      ]),
    );
    expect(counts).toEqual({ writes: 0, restores: 1, removes: 1, backups: 1 });
  });
});

describe("console session plan labels", () => {
  it("labels a file by the direction of the toggle", () => {
    const written = file("/etc/a.conf", "write");
    const created = file("/etc/b.conf", "write", false);
    expect(plannedFileKind(written, true)).toBe("write");
    expect(plannedFileKind(written, false)).toBe("restore");
    expect(plannedFileKind(created, true)).toBe("write");
    expect(plannedFileKind(file("/etc/c.conf", "remove", false), false)).toBe("remove");
  });
});

describe("console session refusal and readiness", () => {
  it("shows the manager name only when one was detected", () => {
    expect(managerLabel(status())).toBe("SDDM");
    expect(managerLabel(status({ manager: null, managerReason: "No display manager." }))).toBeNull();
  });

  it("cannot toggle without a manager, polkit or Linux", () => {
    expect(canToggle(status())).toBe(true);
    expect(canToggle(status({ manager: null }))).toBe(false);
    expect(canToggle(status({ pkexecAvailable: false }))).toBe(false);
    expect(canToggle(status({ supported: false }))).toBe(false);
    // An enabled toggle stays actionable even if detection now fails, so the
    // user is never locked into the console boot.
    expect(canToggle(status({ enabled: true, manager: null }))).toBe(true);
    expect(canToggle(status({ enabled: true, pkexecAvailable: false }))).toBe(false);
  });

  it("selects the sentence the toggle needs", () => {
    const refusal = status({
      manager: null,
      managerReason: "Ludex found no display manager (SDDM, GDM, LightDM or greetd) on this machine.",
    });
    expect(toggleExplanation(refusal)).toEqual({
      kind: "refusal",
      reason: "Ludex found no display manager (SDDM, GDM, LightDM or greetd) on this machine.",
    });
    expect(toggleExplanation(status({ pkexecAvailable: false }))).toEqual({ kind: "polkit" });
    expect(toggleExplanation(status({ enabled: true }))).toEqual({
      kind: "enabled",
      manager: "SDDM",
    });
    expect(toggleExplanation(status({ enabled: true, manager: null }))).toEqual({
      kind: "enabled",
      manager: "ludex-console",
    });
    expect(toggleExplanation(status())).toEqual({ kind: "ready", manager: "SDDM" });
    expect(toggleExplanation(status({ supported: false, message: "Linux only." }))).toEqual({
      kind: "unsupported",
      reason: "Linux only.",
    });
  });

  it("warns when the seat is not an active Wayland session", () => {
    expect(needsSeatAttention(status())).toBe(false);
    expect(
      needsSeatAttention(
        status({ seat: { sessionType: "x11", active: true, compatible: false, note: "x11" } }),
      ),
    ).toBe(true);
    expect(needsSeatAttention(status({ supported: false }))).toBe(false);
  });
});

describe("session entry row", () => {
  it("offers install, remove or a conflict from the entry state", () => {
    expect(entryAction("missing")).toBe("install");
    expect(entryAction("installed")).toBe("remove");
    expect(entryAction("differs")).toBe("conflict");
  });
});
