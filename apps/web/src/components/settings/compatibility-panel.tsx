import { useQuery } from "@tanstack/react-query";
import { RefreshCw } from "lucide-react";
import { native } from "@/lib/native";
import type { LocalSettings } from "@/lib/types";
import { Button, Input } from "@/components/ui/controls";
import { Select } from "@/components/ui/select";
import { SectionHeader } from "@/components/page";

/**
 * Settings → Compatibility (#90).
 *
 * Shows what the scan found (Steam roots, Proton builds, UMU, Wine,
 * winetricks, protonfixes) and lets the user override any path. Overrides are
 * part of `LocalSettings`, so the desktop Save button persists them.
 */
export function CompatibilityPanel({
  draft,
  update,
}: {
  draft: LocalSettings;
  update: (patch: Partial<LocalSettings>) => void;
}) {
  const scan = useQuery({
    queryKey: ["compat-scan"],
    queryFn: () => native.compatScan(),
    staleTime: 60_000,
    retry: false,
  });

  const tools = scan.data?.tools ?? [];
  const proton = tools.filter((tool) => tool.kind === "proton");

  return (
    <section className="mt-10" aria-labelledby="compat-heading">
      <SectionHeader
        title="Compatibility"
        description="Steam, Proton, UMU and Wine detection for Linux launches"
        actions={
          <Button
            variant="ghost"
            size="sm"
            icon={<RefreshCw className="size-3.5" />}
            loading={scan.isFetching}
            onClick={() => void scan.refetch()}
          >
            Rescan
          </Button>
        }
      />
      <div id="compat-heading" className="border-t border-line pt-4">
        {scan.isError && (
          <p role="alert" className="mb-3 text-[12.5px] text-danger">
            The compatibility scan failed. The desktop app reports the reason in its log.
          </p>
        )}

        <div className="flex flex-wrap items-baseline gap-x-5 gap-y-1 font-mono text-[10.5px] text-ink-3">
          <span>{scan.data?.platform ?? "detecting…"}</span>
          {scan.data?.linux && <span>Linux compatibility active</span>}
          <span>
            Steam roots: {scan.data ? scan.data.steamRoots.length : "…"}
          </span>
          <span>{tools.length} tools</span>
        </div>

        {scan.data && (
          <>
            <dl className="mt-4 grid gap-x-8 gap-y-2 sm:grid-cols-2">
              {[
                ["UMU", scan.data.umu],
                ["Proton", scan.data.proton],
                ["Wine", scan.data.wine],
                ["winetricks", scan.data.winetricks],
                ["protonfixes", scan.data.protonfixes],
              ].map(([label, value]) => (
                <div key={label} className="flex items-baseline justify-between gap-4 border-b border-line py-2">
                  <dt className="shrink-0 text-[12px] text-ink-3">{label}</dt>
                  <dd className={value ? "truncate font-mono text-[10.5px] text-ink-2" : "text-[12px] text-ink-3"}>
                    {value ? <span title={value}>{value}</span> : "not detected"}
                  </dd>
                </div>
              ))}
            </dl>

            {scan.data.steamRoots.length > 0 && (
              <p className="mt-2 truncate font-mono text-[10.5px] text-ink-3" title={scan.data.steamRoots.join("\n")}>
                Steam: {scan.data.steamRoots.join(" · ")}
              </p>
            )}

            {tools.length > 0 && (
              <ul className="mt-4 border-t border-line">
                {tools.map((tool) => (
                  <li key={tool.id} className="flex flex-wrap items-baseline gap-x-3 gap-y-1 border-b border-line py-2">
                    <span className="w-[52px] shrink-0 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-3">
                      {tool.kind}
                    </span>
                    <span className="text-[12.5px] text-ink">{tool.name}</span>
                    {tool.version && tool.version !== tool.name && (
                      <span className="font-mono text-[10.5px] text-ink-3">{tool.version}</span>
                    )}
                    <span className="font-mono text-[10.5px] text-ink-3">{tool.source}</span>
                    <span className="ml-auto max-w-[46ch] truncate font-mono text-[10.5px] text-ink-3" title={tool.path}>
                      {tool.path}
                    </span>
                  </li>
                ))}
              </ul>
            )}

            {proton.length > 1 && (
              <p className="mt-2 text-[11.5px] leading-relaxed text-ink-3">
                GE-Proton builds rank above Valve's Experimental and stable builds when Auto picks a tool.
              </p>
            )}

            <div className="mt-6 grid gap-5 sm:grid-cols-2">
              <label className="flex flex-col gap-1.5">
                <span className="text-[13px] font-medium text-ink">Default launch mode</span>
                <Select
                  label="Default launch mode"
                  value={draft.defaultLaunchMode}
                  onChange={(defaultLaunchMode) => update({ defaultLaunchMode })}
                  options={[
                    { value: "auto", label: "Auto", hint: "Best tool per file" },
                    { value: "native", label: "Native", hint: "No compatibility layer" },
                    { value: "proton", label: "Proton", hint: "Proton only" },
                    { value: "umu", label: "UMU", hint: "umu-run only" },
                  ]}
                />
                <span className="text-[11.5px] text-ink-3">
                  New games start with this profile; each game can override it.
                </span>
              </label>

              <div className="flex flex-col gap-1.5">
                <span className="text-[13px] font-medium text-ink">Steam client path</span>
                <p className="truncate font-mono text-[10.5px] text-ink-3" title={scan.data.steamClientPath ?? ""}>
                  {scan.data.steamClientPath ?? "not detected"}
                </p>
                <span className="text-[11.5px] text-ink-3">
                  Used as <span className="font-mono">STEAM_COMPAT_CLIENT_INSTALL_PATH</span>.
                </span>
              </div>
            </div>

            <div className="mt-6 grid gap-5 sm:grid-cols-3">
              <OverrideField
                label="UMU override"
                hint="umu-run binary or its folder"
                placeholder="/usr/bin/umu-run"
                value={draft.compatUmuPath}
                onChange={(compatUmuPath) => update({ compatUmuPath })}
              />
              <OverrideField
                label="Proton override"
                hint="Proton folder or its proton script"
                placeholder="/home/me/.steam/steam/compatibilitytools.d/GE-Proton9-20"
                value={draft.compatProtonPath}
                onChange={(compatProtonPath) => update({ compatProtonPath })}
              />
              <OverrideField
                label="Wine override"
                hint="wine binary"
                placeholder="/usr/bin/wine"
                value={draft.compatWinePath}
                onChange={(compatWinePath) => update({ compatWinePath })}
              />
            </div>

            {scan.data.warnings.length > 0 && (
              <ul className="mt-4 flex flex-col gap-1 text-[11.5px] text-ink-3">
                {scan.data.warnings.map((warning) => (
                  <li key={warning} className="border-l-2 border-line-2 pl-2">
                    {warning}
                  </li>
                ))}
              </ul>
            )}

            <p className="mt-4 text-[11.5px] leading-relaxed text-ink-3">
              Overrides are saved with the desktop settings. Nothing is required: a missing tool only disables the
              launches that need it.
            </p>
          </>
        )}
      </div>
    </section>
  );
}

function OverrideField({
  label,
  hint,
  placeholder,
  value,
  onChange,
}: {
  label: string;
  hint: string;
  placeholder: string;
  value: string;
  onChange: (value: string) => void;
}) {
  return (
    <label className="flex flex-col gap-1.5">
      <span className="text-[13px] font-medium text-ink">{label}</span>
      <Input
        value={value}
        onChange={(event) => onChange(event.target.value)}
        placeholder={placeholder}
        spellCheck={false}
        className="h-10 font-mono text-[11.5px]"
      />
      <span className="text-[11.5px] text-ink-3">{hint}</span>
    </label>
  );
}
