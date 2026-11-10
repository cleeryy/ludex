import { useState } from "react";
import { useParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { Download, Link2, Lock } from "lucide-react";
import { assetUrl, getApiBase } from "@/lib/api";
import { ApiError, errorMessage } from "@/lib/http";
import { accessShare, getPublicShare, type ShareView } from "@/lib/shares";
import { formatBytes, formatDate, formatRating, formatYear } from "@/lib/format";
import { GameBackdrop, GameCover } from "@/components/art/cover";
import { Wordmark } from "@/components/brand";
import { SectionHeader } from "@/components/page";
import { Badge, Button, Field, Input, Skeleton } from "@/components/ui/controls";

/**
 * Public share page (#9): reachable logged out at `#/share/{token}`.
 *
 * It only ever renders the metadata the server puts in `ShareView`; there is no
 * library navigation, no account data and no auth gate around it.
 */
export function ShareScreen() {
  const { token } = useParams<{ token: string }>();
  const shareToken = token ?? "";
  const view = useQuery({
    queryKey: ["public-share", shareToken],
    queryFn: () => getPublicShare(shareToken),
    enabled: shareToken.length > 0,
    // A bad link is a final answer, not a transient failure: show it at once.
    retry: false,
  });

  if (!shareToken || view.isPending) return <ShareLoading />;
  if (view.isError || !view.data) {
    return <ShareUnavailable error={view.error} onRetry={() => void view.refetch()} />;
  }

  const { game, version } = view.data;

  return (
    <div className="min-h-dvh bg-bg">
      <header className="page flex flex-wrap items-center justify-between gap-3 py-5">
        <Wordmark height={18} />
        <span className="flex items-center gap-2 font-mono text-[10px] uppercase tracking-[0.16em] text-ink-3">
          <Link2 className="size-3.5" aria-hidden="true" />
          Shared link
        </span>
      </header>

      <main>
        <section className="force-dark relative isolate overflow-hidden bg-[#0a0a09]">
          <GameBackdrop title={game.title} backgroundUrl={game.backgroundUrl} className="absolute inset-0 h-full" />

          <div className="page relative pb-14 pt-4">
            <p className="font-mono text-[10px] uppercase tracking-[0.16em] text-ink-3">Account-free download</p>

            <div className="mt-[8vh] grid gap-8 lg:grid-cols-[250px_1fr] lg:gap-12 xl:grid-cols-[280px_1fr]">
              <div className="w-[150px] sm:w-[190px] lg:w-full">
                <div className="shadow-[0_40px_80px_-30px_rgba(0,0,0,0.8)]">
                  <GameCover
                    title={game.title}
                    coverUrl={game.coverUrl}
                    index={game.id}
                    year={formatYear(game.releaseDate)}
                    genre={game.genres[0]}
                  />
                </div>
              </div>

              <div className="min-w-0">
                <div className="flex flex-wrap items-center gap-x-3 gap-y-2 font-mono text-[11px] uppercase tracking-[0.08em] text-ink-3">
                  <span>{formatYear(game.releaseDate)}</span>
                  {game.ageRating && (
                    <>
                      <span className="h-3 w-px bg-line-2" aria-hidden="true" />
                      <span>{game.ageRating}+</span>
                    </>
                  )}
                  {game.genres.length > 0 && (
                    <>
                      <span className="h-3 w-px bg-line-2" aria-hidden="true" />
                      <span>{game.genres.join(" / ")}</span>
                    </>
                  )}
                  {version.earlyAccess && <Badge tone="signal">Early access</Badge>}
                </div>

                <h1 className="mt-3 max-w-[18ch] text-[clamp(2.4rem,4.6vw,4.25rem)] font-semibold leading-[0.94] tracking-[-0.035em]">
                  {game.title}
                </h1>

                <div className="mt-5 flex flex-wrap items-center gap-x-5 gap-y-2 text-[12.5px] text-ink-2">
                  {formatRating(game.rating) && (
                    <span className="flex items-baseline gap-1.5">
                      <span className="num text-[15px] text-ink">{formatRating(game.rating)}</span>
                      <span className="text-[11px] text-ink-3">/ 100</span>
                    </span>
                  )}
                  <span className="num">{formatBytes(version.size)}</span>
                  <span>{version.version ? `Version ${version.version}` : "Unversioned build"}</span>
                  {version.releaseDate && <span>Released {formatDate(version.releaseDate)}</span>}
                </div>

                {game.description && <p className="measure mt-6 text-[14.5px] leading-relaxed text-ink-2">{game.description}</p>}
              </div>
            </div>
          </div>
        </section>
        <div className="relative z-10 -mt-16 h-16 bg-gradient-to-b from-transparent to-bg" aria-hidden="true" />

        <div className="page pt-8">
          <div className="grid gap-12 lg:grid-cols-[1fr_360px]">
            <div>
              <section aria-labelledby="details-heading">
                <SectionHeader title="Details" />
                <dl id="details-heading" className="border-t border-line">
                  {[
                    ["Developer", game.developers.join(", ") || "—"],
                    ["Publisher", game.publishers.join(", ") || "—"],
                    ["Released", formatDate(game.releaseDate)],
                    ["Genres", game.genres.join(", ") || "—"],
                    ["Age rating", game.ageRating ? `${game.ageRating}+` : "Not rated"],
                  ].map(([label, value]) => (
                    <div key={label} className="flex items-baseline justify-between gap-4 border-b border-line py-2.5">
                      <dt className="shrink-0 text-[12px] text-ink-3">{label}</dt>
                      <dd className="text-right text-[12.5px] text-ink-2">{value}</dd>
                    </div>
                  ))}
                </dl>
              </section>

              {game.screenshots.length > 0 && (
                <section className="mt-12" aria-labelledby="screenshots-heading">
                  <SectionHeader title="Screenshots" count={game.screenshots.length} />
                  <div id="screenshots-heading" className="grid gap-3 border-t border-line pt-4 sm:grid-cols-2">
                    {game.screenshots.slice(0, 4).map((shot, index) => (
                      <img
                        key={shot}
                        src={assetUrl(shot) ?? ""}
                        alt={`${game.title} screenshot ${index + 1}`}
                        loading="lazy"
                        className="aspect-video w-full border border-line object-cover"
                        onError={(event) => {
                          event.currentTarget.style.display = "none";
                        }}
                      />
                    ))}
                  </div>
                </section>
              )}
            </div>

            <aside className="lg:sticky lg:top-6 lg:self-start">
              <ShareAccessPanel token={shareToken} view={view.data} />
            </aside>
          </div>
        </div>
      </main>

      <footer className="page mt-12 flex flex-wrap items-center justify-between gap-3 border-t border-line py-6 text-[11px] leading-relaxed text-ink-3">
        <span>Served by Ludex. Anyone with this link can open this page; the library owner can revoke it at any time.</span>
        <span className="font-mono uppercase tracking-[0.12em]">No account needed</span>
      </footer>
    </div>
  );
}

/* ------------------------------ access panel ----------------------------- */

const ACCESS_ERROR_TITLES: Record<string, string> = {
  invalid_share_password: "That password didn’t work",
  share_expired: "This link has expired",
  share_exhausted: "The download limit was reached",
  share_not_found: "This link is no longer available",
  rate_limited: "Too many attempts",
  network_error: "Cannot reach the server",
};

function ShareAccessPanel({ token, view }: { token: string; view: ShareView }) {
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ApiError | null>(null);
  const [started, setStarted] = useState(false);

  const { hasPassword, remainingDownloads, expiresAt, version } = view;
  const exhausted = remainingDownloads !== null && remainingDownloads <= 0;

  const access = async () => {
    if (busy || exhausted) return;
    setBusy(true);
    setError(null);
    try {
      const result = await accessShare(token, hasPassword ? password : undefined);
      setStarted(true);
      window.location.assign(`${getApiBase()}${result.url}`);
    } catch (cause) {
      setError(cause instanceof ApiError ? cause : new ApiError(0, "request_failed", errorMessage(cause)));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="border border-line-2 bg-surface p-5">
      <p className="font-mono text-[10px] uppercase tracking-[0.14em] text-ink-3">Download</p>
      <h2 className="mt-1 flex items-center gap-2 text-[22px] font-semibold leading-tight tracking-[-0.02em]">
        {version.version ? `Version ${version.version}` : "Latest version"}
        {version.earlyAccess && <Badge tone="signal">Early access</Badge>}
      </h2>

      <dl className="mt-4 border-t border-line">
        {[
          ["Size", formatBytes(version.size)],
          ["Released", version.releaseDate ? formatDate(version.releaseDate) : "—"],
          ["Link expires", expiresAt ? formatDate(expiresAt) : "Does not expire"],
          ["Downloads left", remainingDownloads === null ? "Unlimited" : String(remainingDownloads)],
        ].map(([label, value]) => (
          <div key={label} className="flex items-baseline justify-between gap-4 border-b border-line py-2.5">
            <dt className="text-[12px] text-ink-3">{label}</dt>
            <dd className="num text-[12.5px] text-ink-2">{value}</dd>
          </div>
        ))}
      </dl>

      <form
        className="mt-4 flex flex-col gap-3"
        onSubmit={(event) => {
          event.preventDefault();
          void access();
        }}
      >
        {hasPassword && (
          <Field label="Password" htmlFor="share-password" hint="Set by the person who shared this link.">
            <Input
              id="share-password"
              type="password"
              autoComplete="off"
              autoFocus
              required
              value={password}
              onChange={(event) => setPassword(event.target.value)}
              placeholder="Enter the share password"
              disabled={busy || exhausted}
            />
          </Field>
        )}

        <Button
          type="submit"
          variant="primary"
          size="lg"
          className="w-full"
          loading={busy}
          disabled={exhausted}
          icon={hasPassword ? <Lock className="size-4" /> : <Download className="size-4" />}
        >
          {exhausted ? "Download limit reached" : hasPassword ? "Unlock and download" : "Download"}
        </Button>
      </form>

      {started && (
        <p role="status" className="mt-3 border-l-2 border-ok pl-3 text-[12px] leading-relaxed text-ink-2">
          Your download should start. The download ticket is valid for 15 minutes; reopen this link if it expires.
        </p>
      )}

      {error && (
        <div role="alert" className="mt-3 border-l-2 border-danger pl-3">
          <p className="text-[12.5px] text-danger">{ACCESS_ERROR_TITLES[error.code] ?? "The download did not start"}</p>
          <p className="mt-0.5 text-[11.5px] leading-relaxed text-ink-3">{error.message}</p>
        </div>
      )}

      <p className="mt-4 border-t border-line pt-3 text-[11.5px] leading-relaxed text-ink-3">
        Anyone with this link can view this page and download the archive.
        {hasPassword && " The password gates downloads; it is verified by the server and never shown again."}
        {remainingDownloads !== null && remainingDownloads > 0 && ` This link allows ${remainingDownloads} more download${remainingDownloads === 1 ? "" : "s"}.`}
      </p>
    </div>
  );
}

/* ------------------------------- fallbacks ------------------------------- */

function ShareLoading() {
  return (
    <div className="min-h-dvh bg-bg">
      <header className="page py-5">
        <Wordmark height={18} />
      </header>
      <div className="page mt-8 grid gap-8 lg:grid-cols-[250px_1fr] lg:gap-12">
        <Skeleton className="aspect-[3/4] w-[150px] sm:w-[190px] lg:w-full" />
        <div className="flex flex-col gap-4">
          <Skeleton className="h-3 w-40" />
          <Skeleton className="h-12 w-3/4" />
          <Skeleton className="h-3 w-56" />
          <Skeleton className="mt-4 h-3 w-full max-w-[520px]" />
          <Skeleton className="h-3 w-full max-w-[460px]" />
        </div>
      </div>
      <p className="page mt-10 flex items-center gap-2 font-mono text-[11px] uppercase tracking-[0.14em] text-ink-3" role="status">
        Opening the shared link
      </p>
    </div>
  );
}

function ShareUnavailable({ error, onRetry }: { error: unknown; onRetry: () => void }) {
  const apiError = error instanceof ApiError ? error : null;
  const notFound = apiError?.status === 404;
  const rateLimited = apiError?.status === 429;
  const title = notFound ? "This link is not available" : rateLimited ? "Too many attempts" : "The link didn’t load";

  return (
    <div className="grid min-h-dvh grid-rows-[auto_1fr] bg-bg">
      <header className="page flex flex-wrap items-center justify-between gap-3 py-5">
        <Wordmark height={18} />
        <span className="font-mono text-[10px] uppercase tracking-[0.16em] text-ink-3">Shared link</span>
      </header>
      <main className="page flex items-center pb-24">
        <div className="max-w-[520px]">
          <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-ink-3">Account-free download</p>
          <h1 className="mt-3 text-[clamp(2.2rem,4.4vw,3.4rem)] font-semibold leading-[0.96] tracking-[-0.033em]">{title}</h1>
          <p className="mt-4 border-l-2 border-danger pl-3 text-[14px] leading-relaxed text-ink-2" role="alert">
            {apiError?.message ?? errorMessage(error)}
          </p>
          <p className="mt-4 max-w-[46ch] text-[12.5px] leading-relaxed text-ink-3">
            Share links can be revoked by the library owner, and they may expire or reach a download limit. Ask the person who
            sent it for a new link.
          </p>
          {!notFound && (
            <Button variant="default" className="mt-6" onClick={onRetry}>
              Try again
            </Button>
          )}
          <p className="mt-10 flex items-center gap-1.5 text-[11px] text-ink-3">
            <Lock className="size-3" aria-hidden="true" />
            This page never exposes the rest of the library.
          </p>
        </div>
      </main>
    </div>
  );
}
