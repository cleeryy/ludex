import type {
  HostCheck,
  HostFamily,
  HostInstallOutcome,
  HostInstallPlan,
  HostManualStep,
  HostPackage,
  HostRefusal,
  HostReport,
} from "@/lib/types";

/**
 * Browser-side stand-in for the host preflight (#285). Only used when the mock
 * adapter is on, so Settings → Host and the first-run prompt can be reviewed
 * without a Linux Tauri build. It never pretends to install anything: the
 * execute path resolves with a demo success line and no process runs.
 */

const FAMILY: HostFamily = {
  id: "dnf",
  label: "Fedora and derivatives (dnf)",
  detail: "ID=fedora in /etc/os-release",
  installable: true,
};

const CHECKS: HostCheck[] = [
  {
    id: "gpu",
    label: "Graphics driver",
    status: "ok",
    detail: "Your GPU is driven by amdgpu (PCI vendor 1002).",
    hint: "amdgpu · 1002",
    installable: false,
    packages: [],
    manual: null,
  },
  {
    id: "vulkan",
    label: "Vulkan",
    status: "ok",
    detail: "2 Vulkan drivers are installed.",
    hint: "radeon_icd.x86_64.json · intel_icd.x86_64.json",
    installable: false,
    packages: [],
    manual: null,
  },
  {
    id: "vulkan32",
    label: "32-bit Vulkan",
    status: "tooOld",
    detail:
      "The 32-bit Vulkan loader is installed, but no 32-bit driver ICD is exposed, so 32-bit games fall back to OpenGL.",
    hint: "/usr/lib32/libvulkan.so.1",
    installable: false,
    packages: [],
    manual: null,
  },
  {
    id: "webkitgtk",
    label: "WebKitGTK runtime",
    status: "ok",
    detail: "The WebKitGTK 4.1 runtime the desktop app needs is installed.",
    hint: "/usr/lib64/libwebkit2gtk-4.1.so.0",
    installable: false,
    packages: [],
    manual: null,
  },
  {
    id: "umu",
    label: "UMU",
    status: "missing",
    detail:
      "UMU is not installed. It is the supported way to run a Windows game with a Proton build outside Steam.",
    hint: null,
    installable: true,
    packages: ["umu-launcher"],
    manual: null,
  },
  {
    id: "proton",
    label: "Proton builds",
    status: "ok",
    detail: "1 Proton build is available: GE-Proton9-20.",
    hint: null,
    installable: false,
    packages: [],
    manual: null,
  },
  {
    id: "gamemode",
    label: "GameMode",
    status: "missing",
    detail: "gamemoderun is not installed.",
    hint: null,
    installable: true,
    packages: ["gamemode"],
    manual: null,
  },
  {
    id: "mangohud",
    label: "MangoHud",
    status: "missing",
    detail:
      "The MangoHud Vulkan layer (libMangoHud.so) is not installed, so the performance overlay cannot be shown.",
    hint: null,
    installable: true,
    packages: ["mangohud"],
    manual: null,
  },
  {
    id: "gamescope",
    label: "Gamescope",
    status: "ok",
    detail: "gamescope is installed.",
    hint: "/usr/bin/gamescope",
    installable: false,
    packages: [],
    manual: null,
  },
];

const report = (): HostReport => ({
  checkedAt: new Date().toISOString().replace(/\.\d{3}Z$/, "Z"),
  supported: true,
  ready: false,
  family: FAMILY,
  findings: CHECKS.map((check) => ({ ...check, packages: [...check.packages] })),
  warnings: [],
  durationMs: 180,
});

function plan(findings: string[]): HostInstallPlan {
  const current = report();
  const requested = [...new Set(findings.map((id) => id.trim()).filter(Boolean))];
  const packages: HostPackage[] = [];
  const manual: HostManualStep[] = [];
  const refused: HostRefusal[] = [];

  for (const id of requested) {
    if (id === "gpu") {
      refused.push({
        finding: id,
        reason:
          "Ludex never installs GPU drivers. Update them from your distribution's driver tool.",
      });
      continue;
    }
    const check = current.findings.find((entry) => entry.id === id);
    if (!check) {
      refused.push({ finding: id, reason: "This check has no package recipe." });
      continue;
    }
    if (!check.installable) {
      if (check.manual) {
        manual.push({
          finding: id,
          label: check.label,
          instruction: check.manual.instruction,
          url: check.manual.url,
        });
      }
      continue;
    }
    for (const name of check.packages) {
      if (!packages.some((entry) => entry.package === name)) {
        packages.push({ finding: id, label: check.label, package: name });
      }
    }
  }

  const command = packages.length
    ? ["pkexec", "dnf", "install", "-y", ...packages.map((entry) => entry.package)]
    : [];

  return {
    family: FAMILY,
    command,
    commandLabel: command.join(" "),
    packages,
    manual,
    refused,
    notes: packages.length ? [] : ["Nothing was selected that can be installed."],
  };
}

export const mockHost = {
  report(refresh = false): Promise<HostReport> {
    void refresh;
    return Promise.resolve(report());
  },

  plan(findings: string[]): Promise<HostInstallPlan> {
    return Promise.resolve(plan(findings));
  },

  install(
    findings: string[],
    dryRun: boolean,
    consent: boolean,
  ): Promise<HostInstallOutcome> {
    const built = plan(findings);
    if (dryRun) return Promise.resolve({ plan: built, report: null });
    if (!consent) {
      return Promise.reject(new Error("The consent screen was not accepted."));
    }
    if (!built.command.length) {
      return Promise.reject(new Error("There is nothing to install for the selected checks."));
    }
    return Promise.resolve({
      plan: built,
      report: {
        success: true,
        cancelled: false,
        exitCode: 0,
        output: `Demo adapter: ${built.commandLabel}\nComplete!`,
        command: built.command,
        durationMs: 1_200,
      },
    });
  },
};
