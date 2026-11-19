// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from "vitest";
import {
  dismissHostPrompt,
  hostGaps,
  hostInstallableIds,
  hostPromptDismissed,
} from "@/lib/host";
import type { HostCheck, HostReport } from "@/lib/types";

function check(id: string, status: HostCheck["status"], installable: boolean): HostCheck {
  return {
    id,
    label: id,
    status,
    detail: `${id} is ${status}`,
    hint: null,
    installable,
    packages: installable ? [`${id}-pkg`] : [],
    manual: null,
  };
}

function report(findings: HostCheck[]): HostReport {
  return {
    checkedAt: "2026-11-19T10:00:00Z",
    supported: true,
    ready: false,
    family: {
      id: "pacman",
      label: "Arch Linux and derivatives (pacman)",
      detail: "ID=cachyos",
      installable: true,
    },
    findings,
    warnings: [],
    durationMs: 12,
  };
}

describe("host preflight helpers", () => {
  beforeEach(() => window.localStorage.clear());

  it("counts both missing and too-old findings as gaps", () => {
    const value = report([
      check("gpu", "ok", false),
      check("umu", "missing", true),
      check("vulkan32", "tooOld", false),
      check("proton", "unknown", false),
    ]);
    expect(hostGaps(value).map((finding) => finding.id)).toEqual(["umu", "vulkan32"]);
    expect(hostInstallableIds(value)).toEqual(["umu"]);
  });

  it("offers nothing for a null report", () => {
    expect(hostGaps(null)).toEqual([]);
    expect(hostInstallableIds(undefined)).toEqual([]);
  });

  it("dismisses the first-run prompt per report timestamp", () => {
    const first = report([]);
    const second = { ...first, checkedAt: "2026-11-20T10:00:00Z" };
    expect(hostPromptDismissed(first.checkedAt)).toBe(false);
    dismissHostPrompt(first.checkedAt);
    expect(hostPromptDismissed(first.checkedAt)).toBe(true);
    // A new probe is new information and may prompt again.
    expect(hostPromptDismissed(second.checkedAt)).toBe(false);
  });
});
