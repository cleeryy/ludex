import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Plus, Trash2 } from "lucide-react";
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

const ROLES: { value: Role; label: string }[] = [
  { value: "admin", label: "Admin" },
  { value: "editor", label: "Editor" },
  { value: "user", label: "Member" },
];


export function UsersScreen() {
  const { user: me } = useAuth();
  const { push } = useToast();
  const queryClient = useQueryClient();
  const [createOpen, setCreateOpen] = useState(false);
  const [deleteTarget, setDeleteTarget] = useState<User | null>(null);

  const { data, isPending, isError, error, refetch } = useQuery({
    queryKey: ["users"],
    queryFn: () => api.listUsers(),
  });

  const invalidate = () => void queryClient.invalidateQueries({ queryKey: ["users"] });

  const update = useMutation({
    mutationFn: ({ id, patch }: { id: number; patch: { role?: Role; activated?: boolean; email?: string | null } }) =>
      api.updateUser(id, patch),
    onSuccess: () => invalidate(),
    onError: (cause: Error) => push({ title: "Could not update the user", description: cause.message, tone: "danger" }),
  });

  const remove = useMutation({
    mutationFn: (id: number) => api.deleteUser(id),
    onSuccess: () => {
      push({ title: "User deleted" });
      setDeleteTarget(null);
      invalidate();
    },
    onError: (cause: Error) => push({ title: "Could not delete the user", description: cause.message, tone: "danger" }),
  });

  return (
    <div className="page page-in">
      <PageHeader
        title="Users"
        meta={<span className="num">{data?.length ?? 0} accounts</span>}
        description="Accounts live on this server. Roles decide what people can do; activation decides whether they can sign in."
        actions={
          <Button variant="primary" icon={<Plus className="size-4" />} onClick={() => setCreateOpen(true)}>
            Add user
          </Button>
        }
      />

      {isError && <ErrorState message={errorMessage(error)} onRetry={() => void refetch()} />}

      <div className="mt-8 border-t border-line">
        <div className="hidden grid-cols-[1fr_140px_110px_120px_40px] items-center gap-4 py-2 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-3 md:grid">
          <span>Account</span>
          <span>Role</span>
          <span>Active</span>
          <span>Created</span>
          <span className="sr-only">Actions</span>
        </div>

        {isPending &&
          Array.from({ length: 4 }).map((_, index) => (
            <div key={index} className="flex items-center gap-4 border-t border-line py-3">
              <Skeleton className="size-8" />
              <Skeleton className="h-3 w-32" />
            </div>
          ))}

        {!isPending && !isError && data?.length === 0 && (
          <p className="py-6 text-[13px] text-ink-3">No accounts. Add the first one.</p>
        )}

        {data?.map((user) => (
          <div
            key={user.id}
            className="grid grid-cols-[1fr_auto] items-center gap-x-4 gap-y-3 border-t border-line py-3 md:grid-cols-[1fr_140px_110px_120px_40px]"
          >
            <div className="flex min-w-0 items-center gap-3">
              <span className="grid size-8 shrink-0 place-items-center rounded-[2px] border border-line-2 bg-surface-2 font-mono text-[10px] text-ink-2">
                {initials(user.username)}
              </span>
              <div className="min-w-0">
                <p className="flex items-center gap-2 truncate text-[13.5px] text-ink">
                  {user.username}
                  {user.id === me?.id && <span className="font-mono text-[9.5px] uppercase tracking-[0.08em] text-ink-3">you</span>}
                </p>
                <p className="truncate text-[11.5px] text-ink-3">{user.email ?? "No email"}</p>
              </div>
            </div>

            <div className="w-[120px] md:w-auto">
              <Select
                label={`Role for ${user.username}`}
                value={user.role}
                onChange={(role) => update.mutate({ id: user.id, patch: { role } })}
                options={ROLES}
                className="h-8 text-[12px]"
              />
            </div>

            <div className="flex items-center gap-2">
              <Switch
                checked={user.activated}
                label={`Activate ${user.username}`}
                onCheckedChange={(activated) => update.mutate({ id: user.id, patch: { activated } })}
              />
              <span className="text-[11.5px] text-ink-3 md:hidden">{user.activated ? "Active" : "Disabled"}</span>
            </div>

            <span className="num hidden text-[11px] text-ink-3 md:block">{formatDate(user.createdAt)}</span>

            <div className="flex justify-end">
              <IconButton
                label={`Delete ${user.username}`}
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
        The last administrator cannot be deleted or demoted, and you cannot delete your own account.
      </p>

      <CreateUserDialog open={createOpen} onOpenChange={setCreateOpen} onCreated={invalidate} />
      <ConfirmDialog
        open={Boolean(deleteTarget)}
        onOpenChange={(open) => !open && setDeleteTarget(null)}
        title={`Delete ${deleteTarget?.username ?? "this user"}?`}
        description="Their sessions end immediately. Progress, playtime and bookmarks stay in the database but are no longer reachable."
        confirmLabel="Delete user"
        loading={remove.isPending}
        onConfirm={() => deleteTarget && remove.mutate(deleteTarget.id)}
      />
    </div>
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
    if (username.trim().length < 3) return setError("Use at least three characters for the username.");
    if (password.length < 8) return setError("Use at least eight characters for the password.");
    setBusy(true);
    try {
      await api.createUser({ username: username.trim(), password, email: email.trim() || null, role });
      push({ title: "User created", description: `${username.trim()} can sign in now.`, tone: "success" });
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
      title="Add a user"
      description="They can sign in immediately unless you deactivate the account afterwards."
      footer={
        <>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button variant="primary" form="create-user" type="submit" loading={busy}>
            Create user
          </Button>
        </>
      }
    >
      <form id="create-user" onSubmit={submit} className="flex flex-col gap-4">
        <Field label="Username" htmlFor="new-username">
          <Input id="new-username" value={username} onChange={(event) => setUsername(event.target.value)} autoFocus required />
        </Field>
        <Field label="Email" htmlFor="new-email" hint="Optional. Used for nothing yet; there is no mail server.">
          <Input id="new-email" type="email" value={email} onChange={(event) => setEmail(event.target.value)} />
        </Field>
        <Field label="Password" htmlFor="new-password" hint="At least eight characters.">
          <Input id="new-password" type="password" autoComplete="new-password" value={password} onChange={(event) => setPassword(event.target.value)} required />
        </Field>
        <Field label="Role" htmlFor="new-role">
          <Select label="Role" id="new-role" value={role} onChange={setRole} options={ROLES} />
        </Field>
        <div className="flex flex-wrap gap-2 text-[11.5px] text-ink-3">
          <Badge tone="outline">Admin: everything</Badge>
          <Badge tone="outline">Editor: ingest, scan, metadata</Badge>
          <Badge tone="outline">Member: browse and play</Badge>
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
