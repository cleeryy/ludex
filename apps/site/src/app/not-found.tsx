import Link from "next/link";

export default function NotFound() {
  return (
    <main className="mx-auto flex min-h-[70vh] w-full max-w-2xl flex-col justify-center px-6 py-16">
      <div className="border-t border-[var(--ludex-line)] pt-8">
        <p className="flex items-center gap-2.5 font-mono text-xs uppercase tracking-[0.14em] text-[var(--ludex-ink-3)]">
          <span className="h-[2px] w-7 bg-[var(--ludex-signal)]" aria-hidden="true" />
          Error 404
        </p>
        <h1 className="mt-4 text-4xl font-semibold tracking-[-0.03em] text-[var(--ludex-ink)]">
          This shelf is empty.
        </h1>
        <p className="mt-3 text-sm leading-relaxed text-[var(--ludex-ink-2)]">
          The documentation page or route you asked for does not exist on this site.
        </p>
        <div className="mt-6 flex flex-wrap gap-3">
          <Link
            href="/"
            className="inline-flex h-10 items-center rounded-sm bg-[var(--ludex-signal)] px-4 text-sm font-medium text-[var(--ludex-on-signal)] hover:opacity-90"
          >
            Back to home
          </Link>
          <Link
            href="/docs"
            className="inline-flex h-10 items-center rounded-sm border border-[var(--ludex-line)] bg-[var(--ludex-surface)] px-4 text-sm font-medium text-[var(--ludex-ink)] hover:border-[var(--ludex-line-2)]"
          >
            Browse docs
          </Link>
        </div>
      </div>
    </main>
  );
}
