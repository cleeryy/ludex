import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Check, ChevronDown, CircleAlert, KeyRound, Pencil, Plus, RefreshCw, Server, Trash2, Wifi } from "lucide-react";
import { useAuth } from "@/state/auth";
import { useToast } from "@/state/toasts";
import { listServers, removeServer, saveServer, activateServer, testServer } from "@/lib/servers";
import { parseServerUrl, serverLabelFor, type ServerUrlProblem } from "@/lib/server-storage";
import type { ServerProfile, ServerTest } from "@/lib/types";
import { isNative } from "@/lib/native";
import { cn } from "@/lib/cn";
import { Badge, Button, Field, Input } from "@/components/ui/controls";
import { Dialog } from "@/components/ui/dialog";
import { DropdownMenu, useDropdown, type MenuEntry } from "@/components/ui/select";
import { t } from "@/lib/i18n";

export function useServersQuery() {
  return useQuery({
    queryKey: ["servers"],
    queryFn: listServers,
    staleTime: 15_000,
    retry: false,
  });
}

function urlProblemMessage(problem: ServerUrlProblem): string {
  switch (problem) {
    case "empty":
      return t("Enter the server address.");
    case "scheme":
      return t("The address must start with http:// or https://.");
    case "host":
      return t("The address is missing a host.");
    case "credentials":
      return t("Do not put a user name or password in the address.");
    case "whitespace":
      return t("The address must not contain spaces.");
    case "query":
      return t("Drop the query string or the fragment from the address.");
    case "port":
      return t("That host or port is not valid.");
  }
}

/**
 * Shell switcher (#24): the active server, the other saved ones and a shortcut
 * to manage them. Switching re-boots the session against the selected server.
 */
export function ServerSwitcher() {
  const { retryBoot } = useAuth();
  const { push } = useToast();
  const queryClient = useQueryClient();
  const menu = useDropdown();
  const { data, isFetching, refetch } = useServersQuery();
  const [manageOpen, setManageOpen] = useState(false);
  const [formOpen, setFormOpen] = useState(false);
  const [busyId, setBusyId] = useState<string | null>(null);

  const active = data?.servers.find((server) => server.active) ?? data?.servers[0] ?? null;

  const use = async (id: string) => {
    if (id === active?.id) return;
    setBusyId(id);
    try {
      const result = await activateServer(id);
      queryClient.clear();
      retryBoot();
      if (result.message) push({ title: t("Server changed"), description: result.message, tone: "default" });
    } catch (cause) {
      push({
        title: t("Could not switch server"),
        description: cause instanceof Error ? cause.message : String(cause),
        tone: "danger",
      });
    } finally {
      setBusyId(null);
    }
  };

  const items: MenuEntry[] = [
    { heading: t("Servers") },
    ...(data?.servers ?? []).map((server) => ({
      label: server.active ? `${server.label} · ${t("in use")}` : server.label,
      icon: server.active ? <Check className="size-3.5 text-signal-text" /> : <Server className="size-3.5" />,
      selected: server.active,
      onSelect: () => void use(server.id),
    })),
    "separator" as const,
    {
      label: t("Add a server"),
      icon: <Plus className="size-3.5" />,
      onSelect: () => {
        setFormOpen(true);
        setManageOpen(true);
      },
    },
    {
      label: t("Manage servers"),
      icon: <RefreshCw className="size-3.5" />,
      onSelect: () => {
        void refetch();
        setManageOpen(true);
      },
    },
  ];

  if (isFetching && !data) {
    return <span className="hidden h-9 w-24 animate-pulse bg-surface-2 sm:block" aria-hidden="true" />;
  }

  return (
    <>
      <button
        {...menu.triggerProps}
        aria-label={t("Server: {name}", { name: active?.label ?? t("none") })}
        className="flex h-9 max-w-[180px] items-center gap-1.5 rounded-[2px] px-2 text-[12.5px] text-ink-2 transition-colors hover:bg-surface hover:text-ink"
      >
        <span className={cn("size-1.5 shrink-0 rounded-full", active ? "bg-ok" : "bg-ink-3")} aria-hidden="true" />
        <span className="truncate">{active?.label ?? t("No server")}</span>
        <ChevronDown className="size-3.5 shrink-0 text-ink-3" aria-hidden="true" />
      </button>
      <DropdownMenu {...menu.menuProps} items={items} label={t("Servers")} />
      <ServerManagerDialog
        open={manageOpen}
        onOpenChange={(open) => {
          setManageOpen(open);
          if (!open) setFormOpen(false);
        }}
        initialFormOpen={formOpen}
        busyId={busyId}
        onUse={use}
        list={data}
      />
    </>
  );
}

/** Settings → Servers: the same list without the dropdown. */
export function ServersPanel() {
  const { retryBoot } = useAuth();
  const { push } = useToast();
  const queryClient = useQueryClient();
  const { data, isPending, isError, error, refetch } = useServersQuery();
  const [formOpen, setFormOpen] = useState(false);
  const [editing, setEditing] = useState<ServerProfile | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [confirming, setConfirming] = useState<ServerProfile | null>(null);

  const use = async (id: string) => {
    setBusyId(id);
    try {
      const result = await activateServer(id);
      queryClient.clear();
      retryBoot();
      if (result.message) push({ title: t("Server changed"), description: result.message, tone: "default" });
    } catch (cause) {
      push({
        title: t("Could not switch server"),
        description: cause instanceof Error ? cause.message : String(cause),
        tone: "danger",
      });
    } finally {
      setBusyId(null);
    }
  };

  const remove = async (server: ServerProfile) => {
    setBusyId(server.id);
    try {
      const result = await removeServer(server.id);
      setConfirming(null);
      await refetch();
      if (server.active) {
        queryClient.clear();
        retryBoot();
      }
      push({
        title: t("Server removed"),
        description: result.message ?? t("Its saved token was deleted. Its download history stays on disk."),
        tone: "success",
      });
    } catch (cause) {
      push({
        title: t("Could not remove the server"),
        description: cause instanceof Error ? cause.message : String(cause),
        tone: "danger",
      });
    } finally {
      setBusyId(null);
    }
  };

  return (
    <section className="mt-12 max-w-[980px]" aria-labelledby="servers-heading">
      <div className="flex flex-wrap items-end justify-between gap-4">
        <div>
          <h2 id="servers-heading" className="text-[17px] font-semibold tracking-[-0.02em] text-ink">
            {t("Servers")}
          </h2>
          <p className="mt-1 max-w-[70ch] text-[12.5px] leading-relaxed text-ink-3">
            {t("Keep more than one Ludex server or account. Each server has its own session, installed-game index and download queue.")}
          </p>
        </div>
        <Button
          size="sm"
          icon={<Plus className="size-3.5" />}
          onClick={() => {
            setEditing(null);
            setFormOpen(true);
          }}
        >
          {t("Add server")}
        </Button>
      </div>

      <div className="mt-4 border-t border-line">
        {isPending && <p className="py-4 text-[13px] text-ink-3">{t("Loading servers…")}</p>}
        {isError && (
          <p role="alert" className="py-4 text-[13px] text-danger">
            {error instanceof Error ? error.message : t("The saved servers could not be read.")}
          </p>
        )}
        {(data?.servers ?? []).map((server) => (
          <div key={server.id} className="flex flex-wrap items-center gap-x-3 gap-y-2 border-b border-line py-3">
            <span className={cn("size-1.5 shrink-0 rounded-full", server.active ? "bg-ok" : "bg-ink-3")} aria-hidden="true" />
            <div className="min-w-0 flex-1">
              <p className="flex flex-wrap items-center gap-2 text-[13.5px] text-ink">
                <span className="truncate">{server.label}</span>
                {server.active && <Badge tone="signal">{t("In use")}</Badge>}
                {server.local && <Badge tone="outline">{t("This device")}</Badge>}
                {server.username && <span className="text-[11.5px] text-ink-3">@{server.username}</span>}
              </p>
              <p className="truncate font-mono text-[10.5px] text-ink-3" title={server.url}>
                {server.url}
              </p>
            </div>
            <span
              className={cn("inline-flex items-center gap-1 text-[11px] text-ink-3", server.hasToken && "text-ok")}
              title={server.hasToken ? t("A token is saved for this server.") : t("No saved token; sign in when you switch.")}
            >
              <KeyRound className="size-3" aria-hidden="true" />
              {server.hasToken ? t("token saved") : t("no token")}
            </span>
            <div className="flex shrink-0 items-center gap-1">
              {!server.active && (
                <Button size="sm" variant="default" loading={busyId === server.id} onClick={() => void use(server.id)}>
                  {t("Use")}
                </Button>
              )}
              <Button
                size="sm"
                variant="ghost"
                icon={<Pencil className="size-3.5" />}
                onClick={() => {
                  setEditing(server);
                  setFormOpen(true);
                }}
              >
                {t("Edit")}
              </Button>
              <Button
                size="sm"
                variant="ghost"
                className="text-ink-3"
                icon={<Trash2 className="size-3.5" />}
                onClick={() => setConfirming(server)}
              >
                {t("Remove")}
              </Button>
            </div>
          </div>
        ))}
        {data && data.servers.length === 0 && (
          <p className="py-4 text-[13px] text-ink-3">
            {t("No server is saved yet. Add the address of a Ludex server to get started.")}
          </p>
        )}
      </div>

      {data?.warning && (
        <p className="mt-3 flex items-start gap-2 border-l-2 border-warn/60 pl-3 text-[11.5px] leading-relaxed text-ink-3">
          <CircleAlert className="mt-0.5 size-3.5 shrink-0 text-warn" aria-hidden="true" />
          <span>{data.warning}</span>
        </p>
      )}
      {data && !data.warning && data.tokenStorage === "keychain" && (
        <p className="mt-3 text-[11.5px] text-ink-3">
          {t("Saved tokens live in the OS keychain ({kind}).", { kind: data.keychain ?? "keychain" })}
        </p>
      )}
      {!isNative() && (
        <p className="mt-3 text-[11.5px] leading-relaxed text-ink-3">
          {t("In the browser, each server's session token is kept in this browser's local storage and never shared with the other servers.")}
        </p>
      )}

      <ServerFormDialog
        open={formOpen}
        onOpenChange={(open) => {
          setFormOpen(open);
          if (!open) setEditing(null);
        }}
        editing={editing}
        onSaved={async () => {
          await refetch();
        }}
      />

      <Dialog
        open={Boolean(confirming)}
        onOpenChange={(open) => {
          if (!open) setConfirming(null);
        }}
        title={t("Remove {name}?", { name: confirming?.label ?? "" })}
        description={t("The saved token is deleted and the server leaves this list. Installed games and download history stay on disk.")}
        size="sm"
        footer={
          <div className="flex justify-end gap-2">
            <Button variant="ghost" onClick={() => setConfirming(null)}>
              {t("Cancel")}
            </Button>
            <Button
              variant="danger"
              loading={Boolean(confirming && busyId === confirming.id)}
              onClick={() => confirming && void remove(confirming)}
            >
              {t("Remove server")}
            </Button>
          </div>
        }
      />
    </section>
  );
}

function ServerFormDialog({
  open,
  onOpenChange,
  editing,
  onSaved,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  editing: ServerProfile | null;
  onSaved: () => Promise<void>;
}) {
  const { push } = useToast();
  const [label, setLabel] = useState("");
  const [url, setUrl] = useState("");
  const [username, setUsername] = useState("");
  const [token, setToken] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [testing, setTesting] = useState(false);
  const [result, setResult] = useState<ServerTest | null>(null);

  useEffect(() => {
    if (!open) return;
    setLabel(editing?.label ?? "");
    setUrl(editing?.url ?? "");
    setUsername(editing?.username ?? "");
    setToken("");
    setError(null);
    setResult(null);
  }, [open, editing]);

  const validate = (): string | null => {
    const parsed = parseServerUrl(url);
    if (!parsed.ok) return urlProblemMessage(parsed.problem);
    return null;
  };

  const test = async () => {
    const problem = validate();
    if (problem) {
      setError(problem);
      return;
    }
    setError(null);
    setTesting(true);
    setResult(null);
    try {
      setResult(await testServer(url, token.trim() || null));
    } catch (cause) {
      setResult({
        ok: false,
        status: null,
        version: null,
        latencyMs: 0,
        message: cause instanceof Error ? cause.message : String(cause),
      });
    } finally {
      setTesting(false);
    }
  };

  const save = async () => {
    const problem = validate();
    if (problem) {
      setError(problem);
      return;
    }
    setError(null);
    setSaving(true);
    try {
      await saveServer({
        id: editing?.id ?? null,
        label: label.trim() || serverLabelFor(url),
        url,
        username: username.trim() || null,
        token: token.trim() || null,
      });
      await onSaved();
      push({ title: editing ? t("Server updated") : t("Server added"), tone: "success" });
      onOpenChange(false);
    } catch (cause) {
      const message = cause instanceof Error ? cause.message : String(cause);
      setError(message === "already_saved" ? t("That server is already saved.") : message);
    } finally {
      setSaving(false);
    }
  };

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title={editing ? t("Edit server") : t("Add server")}
      description={t("A label, the server address and an optional token. Testing the connection does not save anything.")}
      footer={
        <div className="flex flex-wrap justify-end gap-2">
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            {t("Cancel")}
          </Button>
          <Button variant="default" loading={testing} icon={<Wifi className="size-3.5" />} onClick={() => void test()}>
            {t("Test connection")}
          </Button>
          <Button variant="primary" loading={saving} onClick={() => void save()}>
            {editing ? t("Save changes") : t("Add server")}
          </Button>
        </div>
      }
    >
      <div className="flex flex-col gap-4">
        <Field label={t("Label")} htmlFor="server-label" hint={t("Shown in the switcher. Optional.")}>
          <Input
            id="server-label"
            value={label}
            onChange={(event) => setLabel(event.target.value)}
            placeholder={t("Living room")}
            maxLength={60}
            autoFocus
          />
        </Field>
        <Field label={t("Address")} htmlFor="server-url" hint={t("https://ludex.example.com")}>
          <Input
            id="server-url"
            value={url}
            onChange={(event) => setUrl(event.target.value)}
            placeholder="https://ludex.example.com"
            spellCheck={false}
            autoComplete="url"
            className="font-mono text-[12.5px]"
          />
        </Field>
        <div className="grid gap-4 sm:grid-cols-2">
          <Field label={t("Account")} htmlFor="server-user" hint={t("Optional, for your own reference.")}>
            <Input
              id="server-user"
              value={username}
              onChange={(event) => setUsername(event.target.value)}
              autoComplete="username"
              maxLength={80}
            />
          </Field>
          <Field label={t("Token")} htmlFor="server-token" hint={t("Optional. Leave empty to sign in later.")}>
            <Input
              id="server-token"
              type="password"
              value={token}
              onChange={(event) => setToken(event.target.value)}
              autoComplete="off"
              spellCheck={false}
              placeholder={t("Paste a session token")}
            />
          </Field>
        </div>

        {result && (
          <p
            role="status"
            className={cn(
              "border-l-2 pl-3 text-[12px] leading-relaxed",
              result.ok ? "border-ok/60 text-ok" : "border-danger/60 text-danger",
            )}
          >
            {result.message}
          </p>
        )}
        {error && (
          <p role="alert" className="border-l-2 border-danger pl-3 text-[12.5px] text-danger">
            {error}
          </p>
        )}
        {isNative() && (
          <p className="text-[11.5px] leading-relaxed text-ink-3">
            {t("This token is saved for the profile, not in the settings file: the desktop uses the OS keychain when available and a local SQLite file otherwise.")}
          </p>
        )}
      </div>
    </Dialog>
  );
}

function ServerManagerDialog({
  open,
  onOpenChange,
  initialFormOpen,
  busyId,
  onUse,
  list,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  initialFormOpen: boolean;
  busyId: string | null;
  onUse: (id: string) => Promise<void>;
  list: import("@/lib/types").ServerList | undefined;
}) {
  const { retryBoot } = useAuth();
  const { push } = useToast();
  const queryClient = useQueryClient();
  const { data, refetch } = useServersQuery();
  const [formOpen, setFormOpen] = useState(initialFormOpen);
  const [editing, setEditing] = useState<ServerProfile | null>(null);
  const [removing, setRemoving] = useState<ServerProfile | null>(null);
  const servers = data ?? list;

  useEffect(() => {
    if (open) setFormOpen(initialFormOpen);
  }, [open, initialFormOpen]);

  const remove = async (server: ServerProfile) => {
    try {
      const result = await removeServer(server.id);
      setRemoving(null);
      await refetch();
      if (server.active) {
        queryClient.clear();
        retryBoot();
      }
      push({
        title: t("Server removed"),
        description: result.message ?? t("Its saved token was deleted."),
        tone: "success",
      });
    } catch (cause) {
      push({
        title: t("Could not remove the server"),
        description: cause instanceof Error ? cause.message : String(cause),
        tone: "danger",
      });
    }
  };

  return (
    <>
      <Dialog
        open={open}
        onOpenChange={onOpenChange}
        title={t("Servers")}
        description={t("Switch between saved Ludex servers, add one, or test its connection.")}
        size="lg"
        footer={
          <div className="flex justify-end">
            <Button
              variant="default"
              icon={<Plus className="size-3.5" />}
              onClick={() => {
                setEditing(null);
                setFormOpen(true);
              }}
            >
              {t("Add server")}
            </Button>
          </div>
        }
      >
        <div className="border-t border-line">
          {(servers?.servers ?? []).map((server) => (
            <div key={server.id} className="flex flex-wrap items-center gap-x-3 gap-y-2 border-b border-line py-3">
              <span className={cn("size-1.5 shrink-0 rounded-full", server.active ? "bg-ok" : "bg-ink-3")} aria-hidden="true" />
              <div className="min-w-0 flex-1">
                <p className="flex flex-wrap items-center gap-2 text-[13px] text-ink">
                  <span className="truncate">{server.label}</span>
                  {server.active && <Badge tone="signal">{t("In use")}</Badge>}
                </p>
                <p className="truncate font-mono text-[10.5px] text-ink-3">{server.url}</p>
              </div>
              <div className="flex shrink-0 items-center gap-1">
                {!server.active && (
                  <Button size="sm" variant="default" loading={busyId === server.id} onClick={() => void onUse(server.id)}>
                    {t("Use")}
                  </Button>
                )}
                <Button
                  size="sm"
                  variant="ghost"
                  icon={<Pencil className="size-3.5" />}
                  onClick={() => {
                    setEditing(server);
                    setFormOpen(true);
                  }}
                >
                  {t("Edit")}
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  className="text-ink-3"
                  icon={<Trash2 className="size-3.5" />}
                  onClick={() => setRemoving(server)}
                >
                  {t("Remove")}
                </Button>
              </div>
            </div>
          ))}
          {(servers?.servers ?? []).length === 0 && (
            <p className="py-4 text-[13px] text-ink-3">{t("No server is saved yet.")}</p>
          )}
        </div>
        {servers?.warning && (
          <p className="mt-3 flex items-start gap-2 border-l-2 border-warn/60 pl-3 text-[11.5px] leading-relaxed text-ink-3">
            <CircleAlert className="mt-0.5 size-3.5 shrink-0 text-warn" aria-hidden="true" />
            <span>{servers.warning}</span>
          </p>
        )}
      </Dialog>

      <ServerFormDialog
        open={formOpen}
        onOpenChange={(next) => {
          setFormOpen(next);
          if (!next) setEditing(null);
        }}
        editing={editing}
        onSaved={async () => {
          await refetch();
        }}
      />

      <Dialog
        open={Boolean(removing)}
        onOpenChange={(next) => {
          if (!next) setRemoving(null);
        }}
        title={t("Remove {name}?", { name: removing?.label ?? "" })}
        description={t("The saved token is deleted. Installed games and download history stay on disk.")}
        size="sm"
        footer={
          <div className="flex justify-end gap-2">
            <Button variant="ghost" onClick={() => setRemoving(null)}>
              {t("Cancel")}
            </Button>
            <Button variant="danger" onClick={() => removing && void remove(removing)}>
              {t("Remove server")}
            </Button>
          </div>
        }
      />
    </>
  );
}
