import { useQuery } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import { PenLine } from "lucide-react";
import { api } from "@/lib/api";
import { formatDate } from "@/lib/format";
import { useIsAdmin } from "@/state/auth";
import { EmptyState, ErrorState, PageHeader } from "@/components/page";
import { Button, Skeleton } from "@/components/ui/controls";

export function NewsScreen() {
  const isAdmin = useIsAdmin();
  const { data, isPending, isError, error, refetch } = useQuery({
    queryKey: ["news"],
    queryFn: () => api.listNews(),
  });

  return (
    <div className="page page-in">
      <PageHeader
        title="News"
        meta={<span className="num">{data?.length ?? 0} posts</span>}
        description="Release notes and notes from the server. Written here, read here; no feed, no comments."
        actions={
          isAdmin ? (
            <Link to="/admin/news">
              <Button variant="default" size="sm" icon={<PenLine className="size-3.5" />}>
                Manage posts
              </Button>
            </Link>
          ) : undefined
        }
      />

      <div className="mt-8 max-w-[760px]">
        {isPending && (
          <div className="flex flex-col gap-8">
            {Array.from({ length: 3 }).map((_, i) => (
              <div key={i} className="grid grid-cols-[1fr] gap-3 border-t border-line pt-5 sm:grid-cols-[120px_1fr]">
                <Skeleton className="h-3 w-24" />
                <div className="space-y-3">
                  <Skeleton className="h-5 w-2/3" />
                  <Skeleton className="h-3 w-full" />
                  <Skeleton className="h-3 w-5/6" />
                </div>
              </div>
            ))}
          </div>
        )}

        {isError && (
          <ErrorState
            message={error instanceof Error ? error.message : "The server did not answer."}
            onRetry={() => void refetch()}
          />
        )}

        {!isPending && !isError && data && data.length === 0 && (
          <EmptyState title="No posts yet" description="Release notes will appear here after the first update." />
        )}

        {!isPending && !isError && data && data.length > 0 && (
          <div className="flex flex-col">
            {data.map((post, index) => (
              <article
                key={post.id}
                className="grid grid-cols-1 gap-x-8 gap-y-2 border-t border-line py-7 first:border-t-0 first:pt-0 sm:grid-cols-[120px_1fr]"
              >
                <div className="flex flex-col gap-1 pt-1">
                  <time dateTime={post.createdAt} className="num text-[11px] text-ink-3">
                    {formatDate(post.createdAt)}
                  </time>
                  <span className="text-[11.5px] text-ink-3">{post.author}</span>
                  {index === 0 && <span className="mt-1 h-px w-5 bg-signal" aria-hidden="true" />}
                </div>
                <div>
                  <h2 className="text-[24px] font-semibold leading-tight tracking-[-0.025em]">{post.title}</h2>
                  <div className="mt-3 flex flex-col gap-3">
                    {post.body.split("\n\n").map((paragraph, i) => (
                      <p key={i} className="measure text-[14px] leading-relaxed text-ink-2">
                        {paragraph}
                      </p>
                    ))}
                  </div>
                </div>
              </article>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
