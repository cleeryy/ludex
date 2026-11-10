import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Eye, EyeOff, Pencil, Plus, RefreshCw, Trash2 } from "lucide-react";
import { api } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { formatDate, initials } from "@/lib/format";
import type { Role, User } from "@/lib/types";
import { useAuth } from "@/state/auth";
import { useToast } from "@/state/toasts";
import { ErrorState, PageHeader } from "@/components/page";
import { Badge, Button, Field, IconButton, Input, Skeleton, Switch } from "@/components/ui/controls";
import { Select } from "@/components/ui/select";
import { ConfirmDialog, Dialog } from "@/components/ui/dialog";
import { tr, t , tn } from "@/lib/i18n";

function roleOptions(): { value: Role; label: string }[] {
  return [
    { value: "admin", label: t("Admin") },
    { value: "editor", label: t("Editor") },
    { value: "user", label: t("Member") },
  ];
}

type UserPatch = {
  username?: string;
  email?: string | null;
  password?: string;
  role?: Role;
  activated?: boolean;
  ageLimit?: number | null;
};

/** A readable label for an issuer URL, keeping the full value as the title. */
function issuerLabel(issuer: string | null): string {
  if (!issuer) return "SSO";
  try {
    return `SSO · ${new URL(issuer).host}`;
  } catch {
    return `SSO · ${issuer}`;
  }
}

function generatePassword(): string {
  const alphabet = "ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789!@#$%&*";
  const bytes = new Uint8Array(20);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (byte) => alphabet[byte % alphabet.length]).join("");
}

function SourceBadge({ user }: { user: User }) {
  if (user.source === "oidc") {
    return (
      <span title={user.oidcIssuer ?? t("Single sign-on account")}>
        <Badge tone="outline">{issuerLabel(user.oidcIssuer)}</Badge>
      </span>
    );
  }
  return <Badge tone="outline">{t("Local")}</Badge>;
}

export function UsersScreen() {
  const { user: me } = useAuth();
  const { push } = useToast();
  const queryClient = useQueryClient();
  const [createOpen, setCreateOpen] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<User | null>(null);
  const [editTarget, setEditTarget] = useState<User | null>(null);

  const { data, isPending, isError, error, refetch } = useQuery({
    queryKey: ["users"],
    queryFn: () => api.listUsers(),
  });

  const invalidate = () => void queryClient.invalidateQueries({ queryKey: ["users"] });

  const update = useMutation({
    mutationFn: ({ id, patch }: { id: number; patch: UserPatch }) => api.updateUser(id, patch),
    onSuccess: () => invalidate(),
    onError: (cause: Error) => push({ title: t("Could not update the user"), description: cause.message, tone: "danger" }),
  });

  const remove = useMutation({
    mutationFn: (id: number) => api.deleteUser(id),
    onSuccess: () => {
      push({ title: t("User deleted") });
      setDeleteTarget(null);
      invalidate();
    },
    onError: (cause: Error) => push({ title: t("Could not delete the user"), description: cause.message, tone: "danger" }),
  });

  return (
    <div className="page page-in">
      <PageHeader
        title={t("Users")}
        meta={<span className="num">{tn(data?.length ?? 0, "1 account", "{count} accounts")}</span>}
        description={t("Accounts live on this server. Roles decide what people can do; activation decides whether they can sign in.")}
        actions={
          <Button variant="primary" icon={<Plus className="size-4" />} onClick={() => setCreateOpen(true)}>
            {t("Add user")}
          </Button>
        }
      />

      {isError && <ErrorState message={errorMessage(error)} onRetry={() => void refetch()} />}

      <div className="mt-8 border-t border-line">
        <div className="hidden grid-cols-[1fr_140px_110px_120px_72px] items-center gap-4 py-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-3 md:grid">
          <span>{t("Account")}</span>
          <span>{t("Role")}</span>
          <span>{t("Active")}</span>
          <span>{t("Created")}</span>
          <span className="sr-only">{t("Actions")}</span>
        </div>

        {isPending &&
          Array.from({ length: 4 }).map((_, index) => (
            <div key={index} className="flex items-center gap-4 border-t border-line py-3">
              <Skeleton className="size-8" />
              <Skeleton className="h-3 w-32" />
            </div>
          ))}

        {!isPending && !isError && data?.length === 0 && (
          <p className="py-6 text-[13px] text-ink-3">{t("No accounts. Add the first one.")}</p>
        )}

        {data?.map((user) => (
          <div
            key={user.id}
            className="grid grid-cols-[1fr_auto] items-center gap-x-4 gap-y-3 border-t border-line py-3 md:grid-cols-[1fr_140px_110px_120px_72px]"
          >
            <div className="flex min-w-0 items-center gap-3">
              <span className="grid size-8 shrink-0 place-items-center rounded-[2px] border border-line-2 bg-surface-2 font-mono text-[10px] text-ink-2">
                {initials(user.username)}
              </span>
              <div className="min-w-0">
                <p className="flex flex-wrap items-center gap-2 truncate text-[13.5px] text-ink">
                  {user.username}
                  {user.id === me?.id && <span className="font-mono text-[9.5px] uppercase tracking-[0.08em] text-ink-3">{t("you")}</span>}
                  <SourceBadge user={user} />
                </p>
                <p className="truncate text-[11.5px] text-ink-3">{user.email ?? t("No email")}</p>
              </div>
            </div>

            <div className="w-[120px] md:w-auto">
              <Select
                label={t("Role for {name}", { name: user.username })}
                value={user.role}
                onChange={(role) => update.mutate({ id: user.id, patch: { role } })}
                options={roleOptions()}
                className="h-8 text-[12px]"
              />
            </div>

            <div className="flex items-center gap-2">
              <Switch
                checked={user.activated}
                label={t("Activate {name}", { name: user.username })}
                onCheckedChange={(activated) => update.mutate({ id: user.id, patch: { activated } })}
              />
              <span className="text-[11.5px] text-ink-3 md:hidden">{user.activated ? t("Active") : t("Disabled")}</span>
            </div>

            <span className="num hidden text-[11px] text-ink-3 md:block">{formatDate(user.createdAt)}</span>

            <div className="flex justify-end gap-1">
              <IconButton
                label={t("Edit {name}", { name: user.username })}
                variant="ghost"
                className="size-8 text-ink-3 hover:text-ink"
                onClick={() => setEditTarget(user)}
              >
                <Pencil className="size-3.5" />
              </IconButton>
              <IconButton
                label={t("Delete {name}", { name: user.username })}
                variant="ghost"
                className="size-8 text-ink-3 hover:text-danger"
                disabled={user.id === me?.id}
                onClick={() => setDeleteTarget(user)}
              >
                <Trash2 className="size-3.5" />
              </IconButton>
            </div>
          </div>
        ))}
      </div>

      <p className="mt-4 text-[11.5px] leading-relaxed text-ink-3">
        {t("Local accounts store their own identity and password. SSO accounts take their username, email and password from the identity provider; only role and activation are editable here. The last administrator cannot be deleted or demoted, and you cannot delete your own account.")}
      </p>

      <CreateUserDialog open={createOpen} onOpenChange={setCreateOpen} onCreated={invalidate} />
      <EditUserDialog
        key={editTarget?.id ?? "none"}
        target={editTarget}
        onOpenChange={(open) => !open && setEditTarget(null)}
        onUpdated={invalidate}
      />
      <ConfirmDialog
        open={Boolean(deleteTarget)}
        onOpenChange={(open) => !open && setDeleteTarget(null)}
        title={t("Delete {name}?", { name: deleteTarget?.username ?? t("this user") })}
        description={
          deleteTarget?.source === "oidc"
            ? `This account signs in through ${deleteTarget.oidcIssuer ?? t("an identity provider")}. Deleting it here removes the local account, progress and playtime; the identity itself stays with the provider.`
            : t("Their sessions end immediately. Progress, playtime and bookmarks stay in the database but are no longer reachable.")
        }
        confirmLabel={t("Delete user")}
        loading={remove.isPending}
        onConfirm={() => deleteTarget && remove.mutate(deleteTarget.id)}
      />
    </div>
  );
}

function EditUserDialog({
  target,
  onOpenChange,
  onUpdated,
}: {
  target: User | null;
  onOpenChange: (open: boolean) => void;
  onUpdated: () => void;
}) {
  const { push } = useToast();
  const sso = target?.source === "oidc";
  const [username, setUsername] = useState(target?.username ?? "");
  const [email, setEmail] = useState(target?.email ?? "");
  const [password, setPassword] = useState("");
  const [showPassword, setShowPassword] = useState(false);
  const [role, setRole] = useState<Role>(target?.role ?? "user");
  const [activated, setActivated] = useState(target?.activated ?? true);
  const [ageLimit, setAgeLimit] = useState(target?.ageLimit?.toString() ?? "");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const reset = () => {
    setUsername(target?.username ?? "");
    setEmail(target?.email ?? "");
    setRole(target?.role ?? "user");
    setActivated(target?.activated ?? true);
    setAgeLimit(target?.ageLimit?.toString() ?? "");
    setPassword("");
    setShowPassword(false);
    setError(null);
  };

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!target) return;
    setError(null);
    const trimmedUsername = username.trim();
    const nextEmail = email.trim() || null;
    const patch: UserPatch = {};
    if (!sso) {
      if (trimmedUsername.length < 3) return setError(t("Use at least three characters for the username."));
      if (password.length > 0 && password.length < 8) return setError(t("Use at least eight characters for the password."));
      if (trimmedUsername !== target.username) patch.username = trimmedUsername;
      if (nextEmail !== target.email) patch.email = nextEmail;
      if (password.length > 0) patch.password = password;
    }
    if (role !== target.role) patch.role = role;
    if (activated !== target.activated) patch.activated = activated;
    const parsedAge = ageLimit.trim() === "" ? null : Number(ageLimit);
    if (parsedAge !== null && (!Number.isInteger(parsedAge) || parsedAge < 0 || parsedAge > 21)) {
      return setError("The age limit must be a whole number between 0 and 21, or empty.");
    }
    if (parsedAge !== target.ageLimit) patch.ageLimit = parsedAge;
    if (Object.keys(patch).length === 0) {
      onOpenChange(false);
      return;
    }
    setBusy(true);
    try {
      await api.updateUser(target.id, patch);
      push({
        title: t("User updated"),
        description: patch.password ? `The password for ${trimmedUsername} was reset.` : `${trimmedUsername} was updated.`,
        tone: "success",
      });
      reset();
      onOpenChange(false);
      onUpdated();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      open={target !== null}
      onOpenChange={(next) => {
        if (!next) reset();
        onOpenChange(next);
      }}
      title={t("Edit {name}", { name: target?.username ?? t("this user") })}
      description={
        sso
          ? t("Identity fields are read-only for SSO accounts. You can still change the local role and activation.")
          : t("Rename the account, change its email, reset its password or adjust access.")
      }
      footer={
        <>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            {t("Cancel")}
          </Button>
          <Button variant="primary" form="edit-user" type="submit" loading={busy}>
            {t("Save changes")}
          </Button>
        </>
      }
    >
      <form id="edit-user" onSubmit={submit} className="flex flex-col gap-4">
        {sso && (
          <p className="border-l-2 border-line-2 pl-3 text-[12.5px] leading-relaxed text-ink-3">
            {tr("This account is managed by {issuer}. Change the username, email or password there; they cannot be edited here.", {
              issuer: <span className="break-all text-ink-2">{target?.oidcIssuer ?? t("an identity provider")}</span>,
            })}
          </p>
        )}
        <Field label={t("Username")} htmlFor="edit-username">
          <Input
            id="edit-username"
            value={username}
            onChange={(event) => setUsername(event.target.value)}
            disabled={sso}
            readOnly={sso}
            autoFocus={!sso}
            required
          />
        </Field>
        <Field
          label={t("Email")}
          htmlFor="edit-email"
          hint={sso ? t("Provided by the identity provider.") : t("Optional. There is no mail server yet.")}
        >
          <Input
            id="edit-email"
            type="email"
            value={email}
            onChange={(event) => setEmail(event.target.value)}
            disabled={sso}
            readOnly={sso}
          />
        </Field>
        {!sso && (
          <Field
            label={t("New password")}
            htmlFor="edit-password"
            hint={t("Leave blank to keep the current password. At least eight characters.")}
            aside={
              <div className="flex items-center gap-1">
                <Button
                  size="sm"
                  variant="quiet"
                  type="button"
                  icon={<RefreshCw className="size-3.5" />}
                  onClick={() => {
                    setPassword(generatePassword());
                    setShowPassword(true);
                  }}
                >
                  {t("Generate")}
                </Button>
                <Button
                  size="sm"
                  variant="quiet"
                  type="button"
                  icon={showPassword ? <EyeOff className="size-3.5" /> : <Eye className="size-3.5" />}
                  onClick={() => setShowPassword((value) => !value)}
                >
                  {showPassword ? t("Hide") : t("Show")}
                </Button>
              </div>
            }
          >
            <Input
              id="edit-password"
              type={showPassword ? "text" : "password"}
              autoComplete="new-password"
              value={password}
              onChange={(event) => setPassword(event.target.value)}
            />
          </Field>
        )}
        <Field label={t("Role")} htmlFor="edit-role">
          <Select label={t("Role")} id="edit-role" value={role} onChange={setRole} options={roleOptions()} />
        </Field>
        <Field
          label="Age limit"
          htmlFor="edit-age-limit"
          hint="Hide games rated above this number (0–21). Leave empty for no restriction. Unrated games stay visible."
        >
          <Input
            id="edit-age-limit"
            type="number"
            min={0}
            max={21}
            inputMode="numeric"
            placeholder="No limit"
            value={ageLimit}
            onChange={(event) => setAgeLimit(event.target.value)}
          />
        </Field>
        <div className="flex items-center justify-between gap-4">
          <div>
            <p className="text-[13px] font-medium text-ink">{t("Activated")}</p>
            <p className="text-[12.5px] text-ink-3">{t("Disabled accounts cannot sign in.")}</p>
          </div>
          <Switch checked={activated} label={t("Activate {name}", { name: target?.username ?? t("this user") })} onCheckedChange={setActivated} />
        </div>
        {error && (
          <p role="alert" className="border-l-2 border-danger pl-3 text-[12.5px] text-danger">
            {error}
          </p>
        )}
      </form>
    </Dialog>
  );
}

function CreateUserDialog({ open, onOpenChange, onCreated }: { open: boolean; onOpenChange: (open: boolean) => void; onCreated: () => void }) {
  const { push } = useToast();
  const [username, setUsername] = useState("");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [role, setRole] = useState<Role>("user");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const reset = () => {
    setUsername("");
    setEmail("");
    setPassword("");
    setRole("user");
    setError(null);
  };

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    setError(null);
    if (username.trim().length < 3) return setError(t("Use at least three characters for the username."));
    if (password.length < 8) return setError(t("Use at least eight characters for the password."));
    setBusy(true);
    try {
      await api.createUser({ username: username.trim(), password, email: email.trim() || null, role });
      push({ title: t("User created"), description: t("{username} can sign in now.", { username: username.trim() }), tone: "success" });
      reset();
      onOpenChange(false);
      onCreated();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) reset();
        onOpenChange(next);
      }}
      title={t("Add a user")}
      description={t("They can sign in immediately unless you deactivate the account afterwards.")}
      footer={
        <>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            {t("Cancel")}
          </Button>
          <Button variant="primary" form="create-user" type="submit" loading={busy}>
            {t("Create user")}
          </Button>
        </>
      }
    >
      <form id="create-user" onSubmit={submit} className="flex flex-col gap-4">
        <Field label={t("Username")} htmlFor="new-username">
          <Input id="new-username" value={username} onChange={(event) => setUsername(event.target.value)} autoFocus required />
        </Field>
        <Field label={t("Email")} htmlFor="new-email" hint={t("Optional. Used for nothing yet; there is no mail server.")}>
          <Input id="new-email" type="email" value={email} onChange={(event) => setEmail(event.target.value)} />
        </Field>
        <Field label={t("Password")} htmlFor="new-password" hint={t("At least eight characters.")}>
          <Input id="new-password" type="password" autoComplete="new-password" value={password} onChange={(event) => setPassword(event.target.value)} required />
        </Field>
        <Field label={t("Role")} htmlFor="new-role">
          <Select label={t("Role")} id="new-role" value={role} onChange={setRole} options={roleOptions()} />
        </Field>
        <div className="flex flex-wrap gap-2 text-[11.5px] text-ink-3">
          <Badge tone="outline">{t("Admin: everything")}</Badge>
          <Badge tone="outline">{t("Editor: ingest, scan, metadata")}</Badge>
          <Badge tone="outline">{t("Member: browse and play")}</Badge>
        </div>
        {error && (
          <p role="alert" className="border-l-2 border-danger pl-3 text-[12.5px] text-danger">
            {error}
          </p>
        )}
      </form>
    </Dialog>
  );
}
