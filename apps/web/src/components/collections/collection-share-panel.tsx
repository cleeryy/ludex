import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Copy, Link2, Lock, Trash2 } from "lucide-react";
import { api } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { formatDate } from "@/lib/format";
import type { Collection, CollectionShareLink } from "@/lib/types";
import { useToast } from "@/state/toasts";
import { Badge, Button, Field, Input } from "@/components/ui/controls";
import { Select } from "@/components/ui/select";
import { Dialog } from "@/components/ui/dialog";
import { t } from "@/lib/i18n";

const EXPIRY_OPTIONS = [
  { value: "0", label: t("Does not expire") },
  { value: "24", label: t("24 hours") },
  { value: "168", label: t("7 days") },
  { value: "720", label: t("30 days") },
];

function shareUrl(token: string): string {
  return `${window.location.origin}${window.location.pathname}#/collection-share/${token}`;
}

/**
 * Owner-only share-link management for one collection (#173): create, copy the
 * one-time URL and revoke. The plaintext token is shown once, right after
 * creation, and never listed again by the API.
 */
export function CollectionSharePanel({ collection }: { collection: Collection }) {
  const queryClient = useQueryClient();
  const { push } = useToast();
  const [open, setOpen] = useState(false);
  const [creating, setCreating] = useState(false);
  const [revoking, setRevoking] = useState<number | null>(null);
  const [expiry, setExpiry] = useState("168");
  const [password, setPassword] = useState("");
  const [label, setLabel] = useState("");
  const [createdToken, setCreatedToken] = useState<string | null>(null);

  const links = useQuery({
    queryKey: ["collection-shares", collection.id],
    queryFn: () => api.collectionShares(collection.id),
    enabled: collection.owned,
  });

  const create = async () => {
    setCreating(true);
    try {
      const link = await api.createCollectionShare(collection.id, {
        expiresInHours: expiry === "0" ? undefined : Number(expiry),
        password: password.trim() || undefined,
        label: label.trim() || undefined,
      });
      setCreatedToken(link.token ?? null);
      setPassword("");
      setLabel("");
      setOpen(false);
      void queryClient.invalidateQueries({ queryKey: ["collection-shares", collection.id] });
      push({ title: t("Share link created"), description: t("Copy it now; the full link is shown only once."), tone: "success" });
    } catch (cause) {
      push({ title: t("Could not create the share link"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setCreating(false);
    }
  };

  const revoke = async (link: CollectionShareLink) => {
    setRevoking(link.id);
    try {
      await api.revokeCollectionShare(collection.id, link.id);
      void queryClient.invalidateQueries({ queryKey: ["collection-shares", collection.id] });
      push({ title: t("Share link revoked"), description: t("It stops working immediately."), tone: "success" });
    } catch (cause) {
      push({ title: t("Could not revoke the share link"), description: errorMessage(cause), tone: "danger" });
    } finally {
      setRevoking(null);
    }
  };

  const copy = async (token: string) => {
    try {
      await navigator.clipboard.writeText(shareUrl(token));
      push({ title: t("Link copied"), tone: "success" });
    } catch {
      push({ title: t("Copy the link manually"), description: shareUrl(token), tone: "danger" });
    }
  };

  const active = (links.data ?? []).filter((link) => !link.revokedAt);

  return (
    <section className="mt-8 border border-line bg-surface/40 p-4" aria-labelledby="collection-share-heading">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex items-center gap-2">
          <Link2 className="size-3.5 text-signal-text" aria-hidden="true" />
          <h2 id="collection-share-heading" className="text-[13px] font-medium text-ink">
            {t("Share link")}
          </h2>
          {active.length > 0 && <Badge tone="outline">{active.length}</Badge>}
        </div>
        <Button
          variant="default"
          size="sm"
          disabled={collection.visibility === "private"}
          onClick={() => setOpen(true)}
        >
          {t("Create link")}
        </Button>
      </div>

      <p className="mt-2 max-w-[70ch] text-[12px] leading-relaxed text-ink-3">
        {collection.visibility === "private"
          ? t("Switch this collection to Link only or Public before creating a share link.")
          : t("Anyone with the link can read the game titles and metadata, logged out. No files, downloads or account data are shared, and revoking the link closes it immediately.")}
      </p>

      {createdToken && (
        <div className="mt-3 border border-signal/50 bg-signal/5 p-3">
          <p className="text-[11.5px] leading-relaxed text-ink-2">{t("Copy this link now. It is shown once.")}</p>
          <div className="mt-2 flex gap-2">
            <Input readOnly value={shareUrl(createdToken)} onFocus={(event) => event.currentTarget.select()} className="h-9 font-mono text-[11px]" />
            <Button variant="primary" size="sm" icon={<Copy className="size-3.5" />} onClick={() => void copy(createdToken)}>
              {t("Copy")}
            </Button>
            <Button variant="ghost" size="sm" onClick={() => setCreatedToken(null)}>
              {t("Dismiss")}
            </Button>
          </div>
        </div>
      )}

      {active.length > 0 && (
        <ul className="mt-3 divide-y divide-line border-t border-line">
          {active.map((link) => (
            <li key={link.id} className="flex flex-wrap items-center gap-3 py-2.5">
              <span className="min-w-0 flex-1">
                <span className="block truncate text-[12.5px] text-ink">
                  {link.label ?? t("Share link #{id}", { id: link.id })}
                </span>
                <span className="mt-0.5 flex flex-wrap items-center gap-x-3 gap-y-1 font-mono text-[10px] text-ink-3">
                  <span>{t("Created {date}", { date: formatDate(link.createdAt) })}</span>
                  <span>{link.expiresAt ? t("Expires {date}", { date: formatDate(link.expiresAt) }) : t("Does not expire")}</span>
                  <span>{t("{count} views", { count: link.viewCount })}</span>
                  {link.hasPassword && (
                    <span className="flex items-center gap-1">
                      <Lock className="size-2.5" aria-hidden="true" />
                      {t("Password")}
                    </span>
                  )}
                </span>
              </span>
              <Button
                variant="ghost"
                size="sm"
                className="text-ink-3 hover:text-danger"
                icon={<Trash2 className="size-3.5" />}
                loading={revoking === link.id}
                onClick={() => void revoke(link)}
              >
                {t("Revoke")}
              </Button>
            </li>
          ))}
        </ul>
      )}

      <Dialog
        open={open}
        onOpenChange={setOpen}
        title={t("Create a share link")}
        description={t("The link works without an account. Disable it here whenever you want.")}
        footer={
          <>
            <Button variant="ghost" onClick={() => setOpen(false)}>
              {t("Cancel")}
            </Button>
            <Button variant="primary" loading={creating} onClick={() => void create()}>
              {t("Create link")}
            </Button>
          </>
        }
      >
        <div className="flex flex-col gap-4">
          <Field label={t("Expires")} htmlFor="share-expiry">
            <Select
              label={t("Expires")}
              value={expiry}
              onChange={setExpiry}
              options={EXPIRY_OPTIONS}
            />
          </Field>
          <Field label={t("Password (optional)")} htmlFor="share-password" hint={t("At least 4 characters. Visitors must enter it before they see the games.")}>
            <Input
              id="share-password"
              type="password"
              autoComplete="new-password"
              value={password}
              onChange={(event) => setPassword(event.target.value)}
              placeholder={t("No password")}
            />
          </Field>
          <Field label={t("Label (optional)")} htmlFor="share-label" hint={t("Only you see this. It is not shown on the public page.")}>
            <Input
              id="share-label"
              value={label}
              maxLength={100}
              onChange={(event) => setLabel(event.target.value)}
              placeholder={t("For the group chat")}
            />
          </Field>
        </div>
      </Dialog>
    </section>
  );
}
