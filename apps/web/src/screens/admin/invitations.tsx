import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Copy, MailPlus, Plus, Trash2 } from "lucide-react";
import { api } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { formatDate } from "@/lib/format";
import type { CreatedInvitation, Invitation, Role } from "@/lib/types";
import { useToast } from "@/state/toasts";
import { Badge, Button, Field, IconButton, Input, Skeleton, Textarea } from "@/components/ui/controls";
import { Select } from "@/components/ui/select";
import { ConfirmDialog, Dialog } from "@/components/ui/dialog";
import { ErrorState, PageHeader } from "@/components/page";
import { t, tr } from "@/lib/i18n";

function roleOptions(): { value: Role; label: string }[] {
  return [
    { value: "user", label: t("Member") },
    { value: "editor", label: t("Editor") },
    { value: "admin", label: t("Admin") },
  ];
}

function statusBadge(invitation: Invitation) {
  switch (invitation.status) {
    case "pending":
      return <Badge tone="ok">{t("Active")}</Badge>;
    case "used":
      return <Badge tone="outline">{t("Used")}</Badge>;
    case "expired":
      return <Badge tone="outline">{t("Expired")}</Badge>;
    case "revoked":
      return <Badge tone="danger">{t("Revoked")}</Badge>;
  }
}

/** Shareable link to the public registration page for this token. */
function inviteUrl(path: string): string {
  if (typeof window === "undefined") return path;
  return `${window.location.origin}/#${path}`;
}

/**
 * Administration → Invitations (#166).
 *
 * Invitation tokens are shown once, at creation: the list only ever exposes
 * metadata. Expired, used and revoked links stay visible so an administrator
 * can see what happened without reading the database.
 */
export function InvitationsScreen() {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const [createOpen, setCreateOpen] = useState(false);
  const [created, setCreated] = useState<CreatedInvitation | null>(null);
  const [revokeTarget, setRevokeTarget] = useState<Invitation | null>(null);

  const [role, setRole] = useState<Role>("user");
  const [email, setEmail] = useState("");
  const [note, setNote] = useState("");
  const [expiresInDays, setExpiresInDays] = useState("7");
  const [maxUses, setMaxUses] = useState("1");
  const [formError, setFormError] = useState<string | null>(null);

  const { data, isPending, isError, error, refetch } = useQuery({
    queryKey: ["invitations"],
    queryFn: () => api.listInvitations(),
  });

  const invalidate = () => void queryClient.invalidateQueries({ queryKey: ["invitations"] });

  const create = useMutation({
    mutationFn: () =>
      api.createInvitation({
        role,
        email: email.trim() || null,
        note: note.trim() || null,
        expiresInDays: Number(expiresInDays) || 7,
        maxUses: Number(maxUses) || 1,
      }),
    onSuccess: (invitation) => {
      setCreated(invitation);
      setCreateOpen(false);
      setEmail("");
      setNote("");
      setFormError(null);
      invalidate();
    },
    onError: (cause: Error) => setFormError(cause.message),
  });

  const revoke = useMutation({
    mutationFn: (id: number) => api.revokeInvitation(id),
    onSuccess: () => {
      setRevokeTarget(null);
      invalidate();
      push({ title: t("Invitation revoked"), tone: "success" });
    },
    onError: (cause: Error) => {
      setRevokeTarget(null);
      push({ title: t("Could not revoke the invitation"), description: errorMessage(cause), tone: "danger" });
    },
  });

  const copy = (value: string, title: string) => {
    void navigator.clipboard
      .writeText(value)
      .then(() => push({ title, tone: "success" }))
      .catch(() => push({ title: t("Could not copy to the clipboard"), tone: "danger" }));
  };

  return (
    <div className="page page-in">
      <PageHeader
        title={t("Invitations")}
        description={t("Create an expiring link that lets someone join this server with the role you choose.")}
        actions={
          <Button variant="primary" icon={<Plus className="size-4" />} onClick={() => setCreateOpen(true)}>
            {t("New invitation")}
          </Button>
        }
      />

      {isPending && (
        <div className="mt-6 flex flex-col gap-2">
          {Array.from({ length: 4 }, (_, index) => (
            <Skeleton key={index} className="h-12 w-full" />
          ))}
        </div>
      )}

      {isError && <ErrorState message={errorMessage(error)} onRetry={() => void refetch()} />}

      {data && data.length === 0 && (
        <div className="mt-10 flex flex-col items-start gap-3 border border-line p-6">
          <MailPlus className="size-5 text-ink-3" aria-hidden="true" />
          <p className="text-[13px] text-ink-2">
            {t("No invitations yet. Create one and share the link by any channel you already trust.")}
          </p>
          <Button variant="default" size="sm" onClick={() => setCreateOpen(true)}>
            {t("Create the first invitation")}
          </Button>
        </div>
      )}

      {data && data.length > 0 && (
        <div className="mt-6 overflow-x-auto">
          <table className="w-full min-w-[720px] border-collapse text-left">
            <thead>
              <tr className="border-b border-line text-[11.5px] uppercase tracking-[0.08em] text-ink-3">
                <th className="py-2 pr-4 font-medium">{t("Invitation")}</th>
                <th className="py-2 pr-4 font-medium">{t("Role")}</th>
                <th className="py-2 pr-4 font-medium">{t("Uses")}</th>
                <th className="py-2 pr-4 font-medium">{t("Expires")}</th>
                <th className="py-2 pr-4 font-medium">{t("Created")}</th>
                <th className="py-2 text-right font-medium">{t("Actions")}</th>
              </tr>
            </thead>
            <tbody>
              {data.map((invitation) => (
                <tr key={invitation.id} className="border-b border-line align-middle">
                  <td className="py-3 pr-4">
                    <div className="flex flex-col gap-1">
                      <span className="flex flex-wrap items-center gap-2 text-[13px] text-ink">
                        {invitation.email ?? invitation.note ?? `#${invitation.id}`}
                        {statusBadge(invitation)}
                      </span>
                      {invitation.email && invitation.note && (
                        <span className="text-[11.5px] text-ink-3">{invitation.note}</span>
                      )}
                      <span className="text-[11.5px] text-ink-3">
                        {invitation.createdByUsername
                          ? tr("by {user}", { user: invitation.createdByUsername })
                          : t("by a deleted account")}
                      </span>
                    </div>
                  </td>
                  <td className="py-3 pr-4 text-[12.5px] text-ink-2">
                    {invitation.role === "admin" ? t("Admin") : invitation.role === "editor" ? t("Editor") : t("Member")}
                  </td>
                  <td className="num py-3 pr-4 text-[12.5px] text-ink-2">
                    {invitation.uses} / {invitation.maxUses}
                  </td>
                  <td className="num py-3 pr-4 text-[12.5px] text-ink-2">{formatDate(invitation.expiresAt)}</td>
                  <td className="num py-3 pr-4 text-[12.5px] text-ink-2">{formatDate(invitation.createdAt)}</td>
                  <td className="py-3 text-right">
                    {invitation.status === "pending" && (
                      <IconButton
                        label={t("Revoke invitation")}
                        onClick={() => setRevokeTarget(invitation)}
                      >
                        <Trash2 className="size-4" />
                      </IconButton>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <Dialog
        open={createOpen}
        onOpenChange={setCreateOpen}
        title={t("New invitation")}
        description={t("The link is shown once. Copy it now and share it with the invitee yourself.")}
        footer={
          <>
            <Button variant="ghost" onClick={() => setCreateOpen(false)}>
              {t("Cancel")}
            </Button>
            <Button variant="primary" loading={create.isPending} onClick={() => create.mutate()}>
              {t("Create invitation")}
            </Button>
          </>
        }
      >
        <div className="flex flex-col gap-4">
          <Field label={t("Role")} htmlFor="invite-role">
            <Select
              id="invite-role"
              label={t("Role")}
              value={role}
              onChange={(value) => setRole(value as Role)}
              options={roleOptions()}
            />
          </Field>
          <Field
            label={t("Email label")}
            htmlFor="invite-email"
            hint={t("Optional. Prefills the account email and helps you recognise the link.")}
          >
            <Input
              id="invite-email"
              type="email"
              value={email}
              onChange={(event) => setEmail(event.target.value)}
              placeholder="name@example.com"
            />
          </Field>
          <Field label={t("Note")} htmlFor="invite-note" hint={t("Optional. Shown to the invitee on the registration page.")}>
            <Textarea
              id="invite-note"
              value={note}
              onChange={(event) => setNote(event.target.value)}
              rows={2}
              maxLength={500}
            />
          </Field>
          <div className="grid gap-4 sm:grid-cols-2">
            <Field label={t("Expires in days")} htmlFor="invite-days">
              <Input
                id="invite-days"
                type="number"
                min={1}
                max={90}
                value={expiresInDays}
                onChange={(event) => setExpiresInDays(event.target.value)}
              />
            </Field>
            <Field label={t("Maximum uses")} htmlFor="invite-uses">
              <Input
                id="invite-uses"
                type="number"
                min={1}
                max={100}
                value={maxUses}
                onChange={(event) => setMaxUses(event.target.value)}
              />
            </Field>
          </div>
          {formError && (
            <p role="alert" className="border-l-2 border-danger pl-3 text-[12.5px] text-danger">
              {formError}
            </p>
          )}
        </div>
      </Dialog>

      <Dialog
        open={created !== null}
        onOpenChange={(open) => {
          if (!open) setCreated(null);
        }}
        title={t("Invitation ready")}
        description={t("This link is not stored in plaintext and cannot be shown again.")}
        footer={
          <Button variant="primary" onClick={() => setCreated(null)}>
            {t("Done")}
          </Button>
        }
      >
        {created && (
          <div className="flex flex-col gap-3">
            <p className="text-[12.5px] leading-relaxed text-ink-3">
              {tr("Valid until {date}, up to {count} uses.", {
                date: <span className="num">{formatDate(created.invitation.expiresAt)}</span>,
                count: <span className="num">{created.invitation.maxUses}</span>,
              })}
            </p>
            <code className="break-all border border-line-2 bg-surface px-3 py-2 font-mono text-[12px] text-ink">
              {inviteUrl(created.path)}
            </code>
            <div className="flex flex-wrap gap-2">
              <Button
                variant="default"
                size="sm"
                icon={<Copy className="size-3.5" />}
                onClick={() => copy(inviteUrl(created.path), t("Invitation link copied"))}
              >
                {t("Copy link")}
              </Button>
              <Button variant="ghost" size="sm" onClick={() => copy(created.token, t("Token copied"))}>
                {t("Copy token only")}
              </Button>
            </div>
          </div>
        )}
      </Dialog>

      <ConfirmDialog
        open={revokeTarget !== null}
        onOpenChange={(open) => {
          if (!open) setRevokeTarget(null);
        }}
        title={t("Revoke this invitation?")}
        description={t("The link stops working immediately. Accounts created with it are not affected.")}
        confirmLabel={t("Revoke")}
        loading={revoke.isPending}
        onConfirm={() => {
          if (revokeTarget) revoke.mutate(revokeTarget.id);
        }}
      />
    </div>
  );
}
