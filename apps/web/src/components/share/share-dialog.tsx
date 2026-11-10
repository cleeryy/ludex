import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Copy, Link2, Lock } from "lucide-react";
import { errorMessage } from "@/lib/http";
import {
  createShare,
  listShares,
  revokeShare,
  type CreateShareInput,
  type ShareLink,
} from "@/lib/shares";
import { formatBytes, formatDate } from "@/lib/format";
import type { GameDetail } from "@/lib/types";
import { useToast } from "@/state/toasts";
import { Badge, Button, Field, Input } from "@/components/ui/controls";
import { ConfirmDialog, Dialog } from "@/components/ui/dialog";
import { Select } from "@/components/ui/select";

type ExpiryChoice = "never" | "1h" | "24h" | "7d" | "30d";

const EXPIRY_OPTIONS: { value: ExpiryChoice; label: string }[] = [
  { value: "never", label: "Never expires" },
  { value: "1h", label: "1 hour" },
  { value: "24h", label: "24 hours" },
  { value: "7d", label: "7 days" },
  { value: "30d", label: "30 days" },
];

const EXPIRY_HOURS: Record<ExpiryChoice, number | undefined> = {
  never: undefined,
  "1h": 1,
  "24h": 24,
  "7d": 24 * 7,
  "30d": 24 * 30,
};

function describeVersion(game: GameDetail, versionId: number | null): string {
  if (versionId === null) return "Latest available";
  const version = game.versions.find((entry) => entry.id === versionId);
  if (!version) return `Version #${versionId}`;
  return version.version ? `v${version.version}` : version.fileName;
}

function shareState(share: ShareLink): { label: string; tone: "ok" | "danger" | "outline" } {
  if (share.revokedAt) return { label: "Revoked", tone: "danger" };
  if (share.expiresAt && new Date(share.expiresAt).getTime() <= Date.now()) return { label: "Expired", tone: "outline" };
  if (share.maxDownloads !== null && share.downloadCount >= share.maxDownloads) {
    return { label: "Limit reached", tone: "danger" };
  }
  return { label: "Active", tone: "ok" };
}

function downloadLabel(share: ShareLink): string {
  const count = `${share.downloadCount} download${share.downloadCount === 1 ? "" : "s"}`;
  if (share.maxDownloads === null) return count;
  return `${count} of ${share.maxDownloads}`;
}

/**
 * Editor-only dialog (#9): creates account-free links for one game, shows the
 * plaintext URL exactly once and manages the existing links for that game.
 */
export function ShareDialog({
  open,
  onOpenChange,
  game,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  game: GameDetail;
}) {
  const { push } = useToast();
  const queryClient = useQueryClient();

  const [versionChoice, setVersionChoice] = useState<string>("latest");
  const [expiry, setExpiry] = useState<ExpiryChoice>("7d");
  const [password, setPassword] = useState("");
  const [maxDownloads, setMaxDownloads] = useState("");
  const [label, setLabel] = useState("");
  const [formError, setFormError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [created, setCreated] = useState<ShareLink | null>(null);
  const [revokeTarget, setRevokeTarget] = useState<ShareLink | null>(null);
  const [revoking, setRevoking] = useState(false);

  // A fresh dialog starts from the defaults; a plaintext token is shown once
  // and is not kept when the dialog closes.
  useEffect(() => {
    if (!open) return;
    setVersionChoice("latest");
    setExpiry("7d");
    setPassword("");
    setMaxDownloads("");
    setLabel("");
    setFormError(null);
    setCreated(null);
    setRevokeTarget(null);
  }, [open]);

  const shares = useQuery({
    queryKey: ["shares"],
    queryFn: listShares,
    enabled: open,
    retry: false,
  });
  const gameShares = (shares.data ?? []).filter((share) => share.gameId === game.id);

  const shareUrl = created?.token
    ? window.location.origin + window.location.pathname + "#/share/" + created.token
    : null;

  const copyLink = async () => {
    if (!shareUrl) return;
    try {
      await navigator.clipboard.writeText(shareUrl);
      push({ title: "Share link copied" });
    } catch {
      push({ title: "Copy failed", description: "Your browser blocked clipboard access. Select the link and copy it by hand.", tone: "danger" });
    }
  };

  // The confirmation is its own dialog on top of this one: do not let the
  // underlying dialog close underneath it.
  const requestOpenChange = (next: boolean) => {
    if (!next && revokeTarget !== null) return;
    onOpenChange(next);
  };

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (creating) return;
    setFormError(null);

    const payload: CreateShareInput = {};
    if (versionChoice !== "latest") payload.versionId = Number(versionChoice);

    const hours = EXPIRY_HOURS[expiry];
    if (hours !== undefined) payload.expiresInHours = hours;

    const passwordValue = password.trim().length === 0 ? undefined : password;
    if (passwordValue !== undefined && [...passwordValue].length < 4) {
      setFormError("Share passwords need at least 4 characters.");
      return;
    }
    if (passwordValue !== undefined) payload.password = passwordValue;

    const maxValue = maxDownloads.trim();
    if (maxValue) {
      const parsed = Number(maxValue);
      if (!Number.isInteger(parsed) || parsed < 1 || parsed > 100_000) {
        setFormError("Max downloads must be a whole number between 1 and 100000.");
        return;
      }
      payload.maxDownloads = parsed;
    }

    const labelValue = label.trim();
    if (labelValue) {
      if ([...labelValue].length > 100) {
        setFormError("Labels are limited to 100 characters.");
        return;
      }
      payload.label = labelValue;
    }

    setCreating(true);
    try {
      const result = await createShare(game.id, payload);
      setCreated(result);
      setPassword("");
      void queryClient.invalidateQueries({ queryKey: ["shares"] });
    } catch (cause) {
      setFormError(errorMessage(cause));
    } finally {
      setCreating(false);
    }
  };

  const revoke = async () => {
    if (!revokeTarget) return;
    setRevoking(true);
    try {
      await revokeShare(revokeTarget.id);
      push({ title: "Share link revoked", description: "It stops working immediately." });
      setRevokeTarget(null);
      void queryClient.invalidateQueries({ queryKey: ["shares"] });
    } catch (cause) {
      push({ title: "Could not revoke the link", description: errorMessage(cause), tone: "danger" });
    } finally {
      setRevoking(false);
    }
  };

  const versionOptions = [
    {
      value: "latest",
      label: "Latest available",
      hint: game.versions[0] ? describeVersion(game, game.versions[0].id) : undefined,
    },
    ...game.versions.map((version) => ({
      value: String(version.id),
      label: version.version ? `v${version.version}` : version.fileName,
      hint: formatBytes(version.size),
    })),
  ];

  return (
    <>
      <Dialog
        open={open}
        onOpenChange={requestOpenChange}
        title="Share this game"
        description="Create an account-free link. Anyone with it can view the game page and download the archive; a password gates downloads. You can revoke it at any time."
        size="lg"
        dismissible={revokeTarget === null}
        footer={
          <Button variant="ghost" disabled={revokeTarget !== null} onClick={() => onOpenChange(false)}>
            Close
          </Button>
        }
      >
        <div className="flex flex-col gap-6">
          {created && shareUrl && (
            <section className="border border-signal/45 bg-signal/5 p-4" aria-labelledby="share-created-heading">
              <h3 id="share-created-heading" className="flex items-center gap-2 text-[13px] font-medium text-signal-text">
                <Link2 className="size-4" aria-hidden="true" />
                Link created
              </h3>
              <p className="mt-1 text-[11.5px] leading-relaxed text-ink-3">
                This is the only time the full link is shown. Ludex stores a hash of the token, so it cannot display it again.
              </p>
              <div className="mt-3 flex flex-col gap-2 sm:flex-row">
                <Input
                  readOnly
                  value={shareUrl}
                  aria-label="Public share link"
                  className="font-mono text-[11px]"
                  onFocus={(event) => event.currentTarget.select()}
                />
                <Button variant="default" icon={<Copy className="size-4" />} onClick={() => void copyLink()} className="shrink-0">
                  Copy link
                </Button>
              </div>
              <p className="mt-2 flex flex-wrap items-baseline gap-x-3 gap-y-1 font-mono text-[10.5px] text-ink-3">
                <span>{created.expiresAt ? `Expires ${formatDate(created.expiresAt)}` : "No expiry"}</span>
                <span>{created.hasPassword ? "Password required" : "No password"}</span>
                <span>{created.maxDownloads === null ? "Unlimited downloads" : `${created.maxDownloads} downloads max`}</span>
              </p>
            </section>
          )}

          <section aria-labelledby="share-new-heading">
            <div className="flex items-baseline justify-between gap-3">
              <h3 id="share-new-heading" className="text-[15px] font-semibold tracking-[-0.01em]">
                New link
              </h3>
              <span className="font-mono text-[10px] uppercase tracking-[0.14em] text-ink-3">{game.title}</span>
            </div>

            <form onSubmit={(event) => void submit(event)} className="mt-3 flex flex-col gap-4">
              <div className="grid gap-4 sm:grid-cols-2">
                <Field label="Version" htmlFor="share-version" hint="“Latest available” follows library updates.">
                  <Select
                    id="share-version"
                    value={versionChoice}
                    onChange={setVersionChoice}
                    options={versionOptions}
                    label="Version to share"
                  />
                </Field>
                <Field label="Link expiry" htmlFor="share-expiry" hint="After this date the link stops working.">
                  <Select
                    id="share-expiry"
                    value={expiry}
                    onChange={setExpiry}
                    options={EXPIRY_OPTIONS}
                    label="Link expiry"
                  />
                </Field>
                <Field
                  label="Password"
                  htmlFor="share-password-new"
                  hint="Optional, at least 4 characters. Visitors need it to download."
                >
                  <Input
                    id="share-password-new"
                    type="password"
                    autoComplete="new-password"
                    value={password}
                    onChange={(event) => setPassword(event.target.value)}
                    placeholder="No password"
                  />
                </Field>
                <Field
                  label="Max downloads"
                  htmlFor="share-max-downloads"
                  hint="Optional cap on successful downloads. Leave empty for unlimited."
                >
                  <Input
                    id="share-max-downloads"
                    type="number"
                    min={1}
                    max={100_000}
                    inputMode="numeric"
                    value={maxDownloads}
                    onChange={(event) => setMaxDownloads(event.target.value)}
                    placeholder="Unlimited"
                  />
                </Field>
                <Field
                  label="Label"
                  htmlFor="share-label"
                  hint="Optional. Only editors can see it; it never appears on the public page."
                  className="sm:col-span-2"
                >
                  <Input
                    id="share-label"
                    value={label}
                    maxLength={100}
                    onChange={(event) => setLabel(event.target.value)}
                    placeholder="For example: review copy for Sam"
                  />
                </Field>
              </div>

              {formError && (
                <p role="alert" className="border-l-2 border-danger pl-3 text-[12.5px] text-danger">
                  {formError}
                </p>
              )}

              <div className="flex flex-wrap items-center gap-3">
                <Button type="submit" variant="primary" loading={creating} icon={<Link2 className="size-4" />}>
                  Create link
                </Button>
                <p className="text-[11.5px] leading-relaxed text-ink-3">
                  Anyone with the link can open it. A password gates downloads, and revoking the link cuts access immediately.
                </p>
              </div>
            </form>
          </section>

          <section className="border-t border-line pt-5" aria-labelledby="share-list-heading">
            <div className="flex items-baseline justify-between gap-3">
              <h3 id="share-list-heading" className="text-[15px] font-semibold tracking-[-0.01em]">
                Links for this game
              </h3>
              <span className="num text-[11.5px] text-ink-3">{gameShares.length}</span>
            </div>

            {shares.isError && (
              <p role="alert" className="mt-3 border-l-2 border-danger pl-3 text-[12.5px] text-danger">
                {errorMessage(shares.error)}
              </p>
            )}

            {shares.isPending && <p className="mt-3 text-[12.5px] text-ink-3">Loading links…</p>}

            {shares.isSuccess && gameShares.length === 0 && (
              <p className="mt-3 text-[12.5px] leading-relaxed text-ink-3">
                No share links for this game yet. Created links stay listed here so you can revoke them later.
              </p>
            )}

            {gameShares.length > 0 && (
              <ul className="mt-3 border-t border-line">
                {gameShares.map((share) => {
                  const state = shareState(share);
                  const version = describeVersion(game, share.versionId);
                  return (
                    <li
                      key={share.id}
                      className="flex flex-col gap-2 border-b border-line py-3 sm:flex-row sm:items-center sm:justify-between sm:gap-4"
                    >
                      <div className="min-w-0">
                        <p className="flex flex-wrap items-center gap-2 text-[13px] text-ink">
                          <span className="truncate">{share.label || version}</span>
                          <Badge tone={state.tone}>{state.label}</Badge>
                          {share.hasPassword && (
                            <Badge tone="outline">
                              <Lock className="mr-1 size-3" aria-hidden="true" />
                              Password
                            </Badge>
                          )}
                        </p>
                        <p className="mt-1 flex flex-wrap items-baseline gap-x-3 gap-y-1 font-mono text-[10.5px] text-ink-3">
                          {share.label && <span>{version}</span>}
                          <span>Created {formatDate(share.createdAt)}</span>
                          <span>{share.expiresAt ? `Expires ${formatDate(share.expiresAt)}` : "No expiry"}</span>
                          <span>{downloadLabel(share)}</span>
                        </p>
                      </div>
                      {!share.revokedAt && (
                        <Button
                          variant="ghost"
                          size="sm"
                          className="shrink-0 text-ink-2"
                          onClick={() => setRevokeTarget(share)}
                        >
                          Revoke
                        </Button>
                      )}
                    </li>
                  );
                })}
              </ul>
            )}
          </section>
        </div>
      </Dialog>

      <ConfirmDialog
        open={revokeTarget !== null}
        onOpenChange={(next) => {
          if (!next) setRevokeTarget(null);
        }}
        title="Revoke this share link?"
        description="Anyone using it loses access immediately. The link cannot be restored; create a new one instead."
        confirmLabel="Revoke link"
        loading={revoking}
        onConfirm={() => void revoke()}
      />
    </>
  );
}
