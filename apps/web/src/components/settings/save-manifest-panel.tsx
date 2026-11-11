import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ExternalLink, RefreshCw, Trash2 } from "lucide-react";
import { errorMessage } from "@/lib/http";
import { installBridge } from "@/lib/install";
import { native } from "@/lib/native";
import { formatBytes, formatDate } from "@/lib/format";
import type { LocalSettings } from "@/lib/types";
import { formatManifestVersion, manifestActions, manifestState, manifestTone } from "@/lib/save-manifest";
import { t } from "@/lib/i18n";
import { useToast } from "@/state/toasts";
import { SectionHeader } from "@/components/page";
import { Badge, Button, Input, Switch } from "@/components/ui/controls";

/**
 * Settings → Saves (#153): the Ludusavi save-path database Ludex manages.
 *
 * A fresh install has no `ludusavi` binary and no manifest, so detection could
 * only report "no manifest". Ludex now downloads the canonical file into its
 * app data folder, checks at most once per 24 h, and keeps the cached copy
 * when a download fails. The manifest is never bundled and never uploaded.
 *
 * Path/URL edits go through the parent draft and are persisted by "Save
 * desktop settings"; Update and Remove act immediately.
 */
export function SaveManifestPanel({
  draft,
  update,
}: {
  draft: LocalSettings;
  update: (patch: Partial<LocalSettings>) => void;
}) {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const [busy, setBusy] = useState<"update" | "remove" | null>(null);

  const status = useQuery({
    queryKey: ["ludusavi-manifest"],
    queryFn: () => installBridge.manifestStatus(),
    retry: false,
    staleTime: 30_000,
  });

  // The startup hook may refresh the database while this page is open.
  useEffect(() => {
    let disposed = false;
    let off: (() => void) | undefined;
    void native.onSaveManifestUpdated((update) => {
      if (!disposed) queryClient.setQueryData(["ludusavi-manifest"], update);
    }).then((unlisten) => {
      if (disposed) unlisten();
      else off = unlisten;
    });
    return () => {
      disposed = true;
      off?.();
    };
  }, [queryClient]);

  const data = status.data ?? null;
  const state = manifestState(data);
  const actions = manifestActions(data);
  const version = formatManifestVersion(data?.version);
  const lastUpdated = data?.updatedAt ? formatDate(data.updatedAt) : t("Never");
  const lastChecked = data?.checkedAt ? formatDate(data.checkedAt) : t("Never");
  const size = data && data.sizeBytes > 0 ? formatBytes(data.sizeBytes) : "—";

  const refresh = async () => {
    setBusy("update");
    try {
      const next = await installBridge.updateManifest();
      queryClient.setQueryData(["ludusavi-manifest"], next);
      push({
        title: next.updated
          ? t("Save database updated")
          : next.notModified
            ? t("Save database already up to date")
            : t("Save database not updated"),
        description: next.message,
        tone: next.updated || next.notModified ? "success" : "danger",
      });
    } catch (cause) {
      push({ title: t("Could not update the save database"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setBusy(null);
    }
  };

  const remove = async () => {
    setBusy("remove");
    try {
      const next = await installBridge.removeManifest();
      queryClient.setQueryData(["ludusavi-manifest"], next);
      push({
        title: t("Cached save database removed"),
        description: t("Save detection now uses a manifest you installed yourself or the paths you added manually."),
        tone: "success",
      });
    } catch (cause) {
      push({ title: t("Could not remove the save database"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setBusy(null);
    }
  };

  return (
    <section aria-labelledby="save-database-heading">
      <SectionHeader
        id="save-database-heading"
        level={3}
        title={t("Save database")}
        description={t("Ludusavi save-path data, downloaded and refreshed by Ludex so save detection works without installing anything.")}
        actions={
          <div className="flex flex-wrap items-center gap-2">
            <Button
              variant="default"
              size="sm"
              icon={<RefreshCw className="size-3.5" />}
              loading={busy === "update"}
              disabled={status.isPending}
              onClick={() => void refresh()}
            >
              {t("Update manifest")}
            </Button>
            <Button
              variant="ghost"
              size="sm"
              icon={<Trash2 className="size-3.5" />}
              loading={busy === "remove"}
              disabled={!actions.canRemove || status.isPending}
              onClick={() => void remove()}
            >
              {t("Remove cached copy")}
            </Button>
          </div>
        }
      />
      <div className="border-t border-line pt-4">
        {status.isError && (
          <p role="alert" className="mb-3 text-[12.5px] text-danger">
            {t("The save database status did not load.")} {errorMessage(status.error)}
          </p>
        )}

        <div className="flex flex-wrap items-center gap-3">
          <Badge tone={status.isPending ? "outline" : manifestTone(state)}>
            {status.isPending
              ? t("Checking…")
              : state === "ready"
                ? t("Ready")
                : state === "stale"
                  ? t("Refresh recommended")
                  : state === "error"
                    ? t("Cached, last check failed")
                    : t("Not downloaded")}
          </Badge>
          <span className="text-[12px] text-ink-3">{data?.message ?? t("Checking…")}</span>
        </div>

        {data?.error && (
          <p role="status" className="mt-2 max-w-[86ch] border-l-2 border-line-2 pl-3 text-[11px] leading-relaxed text-ink-3">
            {data.error}
          </p>
        )}

        <dl className="mt-4 grid gap-x-8 gap-y-2 sm:grid-cols-2">
          <div className="flex items-baseline justify-between gap-4 border-b border-line py-2">
            <dt className="shrink-0 text-[12px] text-ink-3">{t("Version")}</dt>
            <dd className="truncate font-mono text-[10.5px] text-ink-2">{version || t("Unknown")}</dd>
          </div>
          <div className="flex items-baseline justify-between gap-4 border-b border-line py-2">
            <dt className="shrink-0 text-[12px] text-ink-3">{t("Last updated")}</dt>
            <dd className="text-[11.5px] text-ink-2">{lastUpdated}</dd>
          </div>
          <div className="flex items-baseline justify-between gap-4 border-b border-line py-2">
            <dt className="shrink-0 text-[12px] text-ink-3">{t("Last checked")}</dt>
            <dd className="text-[11.5px] text-ink-2">{lastChecked}</dd>
          </div>
          <div className="flex items-baseline justify-between gap-4 border-b border-line py-2">
            <dt className="shrink-0 text-[12px] text-ink-3">{t("Size")}</dt>
            <dd className="text-[11.5px] text-ink-2">{size}</dd>
          </div>
        </dl>

        {data?.path && (
          <p className="mt-3 truncate font-mono text-[10.5px] text-ink-3" title={data.path}>
            {data.path}
          </p>
        )}
        {data?.source && (
          <p className="mt-1 flex items-center gap-1.5 truncate font-mono text-[10.5px] text-ink-3" title={data.source}>
            <ExternalLink className="size-3 shrink-0" aria-hidden />
            {data.source}
          </p>
        )}

        <div className="mt-5 flex flex-wrap items-start justify-between gap-4 border-t border-line pt-4">
          <div>
            <p className="text-[13px] font-medium text-ink">{t("Download updates automatically")}</p>
            <p className="mt-1 max-w-[62ch] text-[11.5px] leading-relaxed text-ink-3">
              {t("Checks at most once per day at startup and keeps the cached copy when the download fails. Turn this off to update only from this page.")}
            </p>
          </div>
          <Switch
            checked={draft.ludusaviManifestAutoUpdate}
            onCheckedChange={(ludusaviManifestAutoUpdate) => update({ ludusaviManifestAutoUpdate })}
            label={t("Download updates automatically")}
          />
        </div>

        <div className="mt-5 grid gap-4 sm:grid-cols-2">
          <label className="flex flex-col gap-1.5">
            <span className="text-[12px] text-ink-2">{t("Download URL")}</span>
            <Input
              value={draft.ludusaviManifestUrl}
              placeholder="https://raw.githubusercontent.com/mtkennerly/ludusavi-manifest/data/data/manifest.yaml"
              onChange={(event) => update({ ludusaviManifestUrl: event.target.value })}
              className="h-10 font-mono text-[11px]"
            />
            <span className="text-[10.5px] leading-relaxed text-ink-3">
              {t("Leave empty for the official manifest. A mirror or pinned copy must serve the same manifest.yaml. Save the settings before updating.")}
            </span>
          </label>
          <label className="flex flex-col gap-1.5">
            <span className="text-[12px] text-ink-2">{t("Manifest path override")}</span>
            <Input
              value={draft.ludusaviManifestPath}
              placeholder={t("Detected automatically")}
              onChange={(event) => update({ ludusaviManifestPath: event.target.value })}
              className="h-10 font-mono text-[11px]"
            />
            <span className="text-[10.5px] leading-relaxed text-ink-3">
              {t("A file you point at wins over the managed copy. Leave empty to use the downloaded database, then a Ludusavi install.")}
            </span>
          </label>
          <label className="flex flex-col gap-1.5">
            <span className="text-[12px] text-ink-2">{t("Ludusavi binary (optional)")}</span>
            <Input
              value={draft.ludusaviBinary}
              placeholder={t("Looked up on PATH")}
              onChange={(event) => update({ ludusaviBinary: event.target.value })}
              className="h-10 font-mono text-[11px]"
            />
            <span className="text-[10.5px] leading-relaxed text-ink-3">
              {t("Advanced: a Ludusavi CLI can preview saves with its own rules. It is only asked when the manifest lists nothing.")}
            </span>
          </label>
        </div>

        <p className="mt-4 max-w-[86ch] border-l-2 border-line-2 pl-3 text-[11px] leading-relaxed text-ink-3">
          {t("The save database comes from the Ludusavi manifest (MIT), which is built from PCGamingWiki data (CC BY-NC-SA). Ludex downloads it at runtime, never bundles it and never sends it to your server. Saves stay on this machine unless you upload a snapshot.")}
        </p>
      </div>
    </section>
  );
}
