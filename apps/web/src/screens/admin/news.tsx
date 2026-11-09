import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import { ArrowUpRight, Plus, Trash2 } from "lucide-react";
import { api } from "@/lib/api";
import { errorMessage } from "@/lib/http";
import { formatDate } from "@/lib/format";
import type { NewsPost } from "@/lib/types";
import { useToast } from "@/state/toasts";
import { EmptyState, ErrorState, PageHeader } from "@/components/page";
import { Button, Field, IconButton, Input, Skeleton, Textarea } from "@/components/ui/controls";
import { ConfirmDialog, Dialog } from "@/components/ui/dialog";

export function AdminNewsScreen() {
  const { push } = useToast();
  const queryClient = useQueryClient();
  const [editor, setEditor] = useState<NewsPost | "new" | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<NewsPost | null>(null);

  const { data, isPending, isError, error, refetch } = useQuery({
    queryKey: ["news"],
    queryFn: () => api.listNews(),
  });

  const invalidate = () => void queryClient.invalidateQueries({ queryKey: ["news"] });

  const remove = useMutation({
    mutationFn: (id: number) => api.deleteNews(id),
    onSuccess: () => {
      push({ title: "Post deleted" });
      setDeleteTarget(null);
      invalidate();
    },
    onError: (cause: Error) => push({ title: "Could not delete the post", description: cause.message, tone: "danger" }),
  });

  return (
    <div className="page page-in">
      <PageHeader
        title="News"
        meta={<span className="num">{data?.length ?? 0} posts</span>}
        description="Posts appear on the News page for every account on this server."
        actions={
          <Button variant="primary" icon={<Plus className="size-4" />} onClick={() => setEditor("new")}>
            New post
          </Button>
        }
      />

      {isError && <ErrorState message={errorMessage(error)} onRetry={() => void refetch()} />}

      <div className="mt-8">
        {isPending &&
          Array.from({ length: 3 }).map((_, index) => (
            <div key={index} className="flex flex-col gap-2 border-b border-line py-4">
              <Skeleton className="h-4 w-1/3" />
              <Skeleton className="h-3 w-1/5" />
            </div>
          ))}

        {!isPending && !isError && data && data.length === 0 && (
          <EmptyState title="No posts yet" description="Write the first note about this server or a release." compact />
        )}

        {data?.map((post) => (
          <div key={post.id} className="flex flex-wrap items-baseline justify-between gap-3 border-b border-line py-4">
            <div className="min-w-0">
              <p className="truncate text-[14.5px] text-ink">{post.title}</p>
              <p className="mt-1 flex flex-wrap items-baseline gap-x-3 font-mono text-[10.5px] text-ink-3">
                <span>{formatDate(post.createdAt)}</span>
                <span>{post.author}</span>
                {post.updatedAt !== post.createdAt && <span>edited {formatDate(post.updatedAt)}</span>}
              </p>
            </div>
            <div className="flex items-center gap-1">
              <Link to="/news" className="mr-1 flex h-8 items-center gap-1 text-[12px] text-ink-3 hover:text-ink">
                View
                <ArrowUpRight className="size-3" aria-hidden="true" />
              </Link>
              <Button variant="ghost" size="sm" onClick={() => setEditor(post)} className="text-ink-2">
                Edit
              </Button>
              <IconButton
                label={`Delete ${post.title}`}
                variant="ghost"
                className="size-8 text-ink-3 hover:text-danger"
                onClick={() => setDeleteTarget(post)}
              >
                <Trash2 className="size-3.5" />
              </IconButton>
            </div>
          </div>
        ))}
      </div>

      <NewsEditor
        value={editor}
        onClose={() => setEditor(null)}
        onSaved={() => {
          invalidate();
          setEditor(null);
        }}
      />

      <ConfirmDialog
        open={Boolean(deleteTarget)}
        onOpenChange={(open) => !open && setDeleteTarget(null)}
        title="Delete this post?"
        description={deleteTarget ? `“${deleteTarget.title}” will be removed from the News page.` : ""}
        confirmLabel="Delete post"
        loading={remove.isPending}
        onConfirm={() => deleteTarget && remove.mutate(deleteTarget.id)}
      />
    </div>
  );
}

function NewsEditor({
  value,
  onClose,
  onSaved,
}: {
  value: NewsPost | "new" | null;
  onClose: () => void;
  onSaved: () => void;
}) {
  const { push } = useToast();
  const isNew = value === "new";
  const post = value && value !== "new" ? value : null;
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [loadedId, setLoadedId] = useState<number | "new" | null>(null);

  // Reset the fields when a different post is opened.
  const currentId = value === "new" ? "new" : post?.id ?? null;
  if (value && currentId !== loadedId) {
    setLoadedId(currentId);
    setTitle(post?.title ?? "");
    setBody(post?.body ?? "");
    setError(null);
  }

  const save = async (event: React.FormEvent) => {
    event.preventDefault();
    setError(null);
    if (!title.trim()) return setError("Give the post a title.");
    setBusy(true);
    try {
      if (post) await api.updateNews(post.id, { title: title.trim(), body });
      else await api.createNews({ title: title.trim(), body });
      push({ title: post ? "Post updated" : "Post published", tone: "success" });
      onSaved();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      open={Boolean(value)}
      onOpenChange={(open) => {
        if (!open) {
          setLoadedId(null);
          onClose();
        }
      }}
      title={isNew ? "New post" : "Edit post"}
      description="Plain text. Blank lines separate paragraphs."
      size="lg"
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" form="news-form" type="submit" loading={busy}>
            {isNew ? "Publish" : "Save changes"}
          </Button>
        </>
      }
    >
      <form id="news-form" onSubmit={save} className="flex flex-col gap-4">
        <Field label="Title" htmlFor="news-title">
          <Input id="news-title" value={title} onChange={(event) => setTitle(event.target.value)} autoFocus required maxLength={120} />
        </Field>
        <Field label="Body" htmlFor="news-body">
          <Textarea id="news-body" value={body} onChange={(event) => setBody(event.target.value)} rows={10} className="min-h-[220px]" />
        </Field>
        {error && (
          <p role="alert" className="border-l-2 border-danger pl-3 text-[12.5px] text-danger">
            {error}
          </p>
        )}
      </form>
    </Dialog>
  );
}
