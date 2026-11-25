import Link from "next/link";
import {
  ArrowRight,
  BookOpen,
  Cloud,
  Cpu,
  Download,
  Gamepad2,
  HardDrive,
  Lock,
  ShieldCheck,
  Terminal,
} from "lucide-react";
import { GITHUB_URL, LUDEX_VERSION, RELEASES_URL, withBase } from "@/lib/site";

const FEATURES = [
  {
    icon: HardDrive,
    title: "One binary, boring deployment",
    body: "A single Rust binary with an embedded React UI and SQLite by default (PostgreSQL and S3 storage optional). Three Docker volumes are the whole install.",
  },
  {
    icon: Download,
    title: "Resilient install transports",
    body: "Classic HTTP Range resume with full SHA-256 checks, ZIP per-entry streaming installs on by default, and parallel file-manifest installs for pre-extracted archives.",
  },
  {
    icon: Gamepad2,
    title: "Desktop, web, PWA & Console 2.0",
    body: "One React codebase styled with the ARC × Swiss/Bauhaus design system (8 themes + custom dual-color), plus an opt-in 10-foot TV shell behind the console_v2 device flag.",
  },
  {
    icon: Cpu,
    title: "First-class Linux gaming",
    body: "Proton, GE-Proton, UMU, Wine, Gamescope, ProtonDB ratings, compatibility rules, and standalone portable bundles (run.sh / install.sh) that launch without Ludex.",
  },
  {
    icon: Cloud,
    title: "Cloud saves & remote installs",
    body: "Manual browser snapshots plus automatic post-session desktop save sync via Ludusavi discovery, and a mobile PWA queue that dispatches installs to your online PCs.",
  },
  {
    icon: ShieldCheck,
    title: "Inspectable security",
    body: "Argon2id password hashes, TOTP 2FA, OIDC SSO, API keys, signed webhooks, VirusTotal scanning, minisign-verified desktop updates, and a documented API contract.",
  },
];

const GALLERY = [
  {
    title: "Home",
    caption: "Continue playing, games installed on this device, collections and recently played titles.",
    src: "/images/screenshots/design-333/home-speranza-desktop.png",
    alt: "Ludex Home screen with continue-playing hero and installed games list",
  },
  {
    title: "Downloads queue",
    caption: "Persisted install queue with speed, ETA, bandwidth limits, pause/resume and disk reservations.",
    src: "/images/screenshots/design-333/downloads-speranza-desktop.png",
    alt: "Ludex Downloads queue with active transfer progress and queued jobs",
  },
  {
    title: "Console 2.0 — Stage (opt-in flag)",
    caption: "10-foot gamepad/TV-remote shell enabled per device via console_v2 (default off). Publishable in Sunshine.",
    src: "/images/screenshots/console/stage-1080.png",
    alt: "Console 2.0 Stage view in 1080p showing featured game and cover rail",
  },
  {
    title: "Console 2.0 — Wall",
    caption: "Full library cover grid under shelf filter chips with 720p/1080p/4K overscan-safe layouts.",
    src: "/images/screenshots/console/wall-1080.png",
    alt: "Console 2.0 Wall view showing cover grid under shelf chips",
  },
  {
    title: "Game detail",
    caption: "Versions and rollback, personal notes, cloud saves, achievements, ProtonDB badge and tiered launch controls.",
    src: "/images/screenshots/readme/game-page.png",
    alt: "Game detail page showing cover, rating, playtime, versions and install controls",
  },
  {
    title: "Admin & Library Health",
    caption: "Library Health Center, games/users/ingest management, VirusTotal verdicts, backups and audit log.",
    src: "/images/screenshots/design-333/admin-health-speranza-desktop.png",
    alt: "Admin Library Health Center with server checks and storage diagnostics",
  },
];

export default function LandingPage() {
  return (
    <main className="mx-auto w-full max-w-6xl px-5 pb-20 pt-10 sm:px-8 sm:pt-14">
      {/* 4-band hardware accent stripe */}
      <div className="mb-8 flex h-1 w-28 overflow-hidden" aria-hidden="true">
        <span className="flex-1 bg-[var(--ludex-stripe-1)]" />
        <span className="flex-1 bg-[var(--ludex-stripe-2)]" />
        <span className="flex-1 bg-[var(--ludex-stripe-3)]" />
        <span className="flex-1 bg-[var(--ludex-stripe-4)]" />
      </div>

      {/* Hero */}
      <section className="grid gap-10 lg:grid-cols-[1.1fr_1fr] lg:items-end">
        <div>
          <div className="mb-4 inline-flex items-center gap-2 rounded-sm border border-[var(--ludex-line)] bg-[var(--ludex-surface)] px-2.5 py-1 font-mono text-xs text-[var(--ludex-ink-2)]">
            <span className="size-1.5 rounded-full bg-[var(--ludex-signal)]" aria-hidden="true" />
            <span className="ludex-num">v{LUDEX_VERSION}</span>
            <span>·</span>
            <span>AGPL-3.0-or-later</span>
            <span>·</span>
            <span>pre-1.0</span>
          </div>

          <h1 className="max-w-[16ch] text-4xl font-semibold leading-[0.98] tracking-[-0.03em] text-[var(--ludex-ink)] sm:text-5xl lg:text-6xl">
            The game library you host yourself.
          </h1>

          <p className="mt-5 max-w-[54ch] text-base leading-relaxed text-[var(--ludex-ink-2)] sm:text-lg">
            Index the DRM-free game archives you already own, enrich them with metadata, and
            install, launch and track playtime from one Rust binary and a Tauri 2 desktop app.
            SQLite by default, no vendor account, no telemetry.
          </p>

          <div className="mt-7 flex flex-wrap items-center gap-3">
            <Link
              href="/docs/getting-started/install"
              className="inline-flex h-11 items-center gap-2 rounded-sm bg-[var(--ludex-signal)] px-5 text-sm font-medium text-[var(--ludex-on-signal)] transition-opacity hover:opacity-90"
            >
              Get started
              <ArrowRight className="size-4" aria-hidden="true" />
            </Link>
            <Link
              href="/docs"
              className="inline-flex h-11 items-center gap-2 rounded-sm border border-[var(--ludex-line)] bg-[var(--ludex-surface)] px-5 text-sm font-medium text-[var(--ludex-ink)] transition-colors hover:border-[var(--ludex-line-2)] hover:bg-[var(--ludex-surface-2)]"
            >
              <BookOpen className="size-4" aria-hidden="true" />
              Read the docs
            </Link>
            <a
              href={RELEASES_URL}
              target="_blank"
              rel="noreferrer"
              className="inline-flex h-11 items-center gap-2 rounded-sm border border-[var(--ludex-line)] px-4 text-sm text-[var(--ludex-ink-2)] transition-colors hover:border-[var(--ludex-line-2)] hover:text-[var(--ludex-ink)]"
            >
              <Download className="size-4" aria-hidden="true" />
              Releases
            </a>
          </div>
        </div>

        <div className="rounded-md border border-[var(--ludex-line)] bg-[var(--ludex-surface)] p-4 sm:p-5">
          <p className="flex items-center gap-2 font-mono text-xs uppercase tracking-[0.12em] text-[var(--ludex-ink-3)]">
            <Terminal className="size-3.5 text-[var(--ludex-signal)]" aria-hidden="true" />
            Docker quick start
          </p>
          <pre className="mt-3 overflow-x-auto rounded-sm border border-[var(--ludex-line)] bg-[var(--ludex-bg-deep)] p-3.5 font-mono text-xs leading-relaxed text-[var(--ludex-ink)]">
            <code>{`git clone ${GITHUB_URL}.git && cd ludex
mkdir -p games media data && sudo chown -R 1000:1000 games media data
LUDEX_IMAGE=ghcr.io/cleeryy/ludex:v${LUDEX_VERSION} docker compose up -d`}</code>
          </pre>
          <p className="mt-3 text-xs leading-relaxed text-[var(--ludex-ink-3)]">
            Prefer zero-server setup? Launch the desktop app and choose{" "}
            <strong className="font-medium text-[var(--ludex-ink-2)]">Local library</strong> to
            run the embedded Rust server on <code className="font-mono">127.0.0.1</code>.
          </p>
        </div>
      </section>

      {/* Main screenshot showcase */}
      <section className="mt-12 overflow-hidden rounded-md border border-[var(--ludex-line)] bg-[var(--ludex-surface)]">
        <img
          src={withBase("/images/screenshots/design-333/library-speranza-desktop.png")}
          alt="Ludex library grid in Speranza dark theme"
          className="hidden w-full dark:block"
        />
        <img
          src={withBase("/images/screenshots/design-333/library-bauhaus-desktop.png")}
          alt="Ludex library grid in Bauhaus light theme"
          className="block w-full dark:hidden"
        />
      </section>

      {/* Features */}
      <section className="mt-16 border-t border-[var(--ludex-line)] pt-12">
        <div className="max-w-[58ch]">
          <p className="font-mono text-xs uppercase tracking-[0.12em] text-[var(--ludex-ink-3)]">
            Architecture & capabilities
          </p>
          <h2 className="mt-2 text-2xl font-semibold tracking-[-0.02em] text-[var(--ludex-ink)] sm:text-3xl">
            Built for archives you own, not vendor lock-in.
          </h2>
        </div>

        <div className="mt-8 grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
          {FEATURES.map(({ icon: Icon, title, body }) => (
            <div
              key={title}
              className="rounded-md border border-[var(--ludex-line)] bg-[var(--ludex-surface)] p-5"
            >
              <Icon className="size-4 text-[var(--ludex-signal)]" aria-hidden="true" />
              <h3 className="mt-3 text-base font-semibold text-[var(--ludex-ink)]">{title}</h3>
              <p className="mt-2 text-sm leading-relaxed text-[var(--ludex-ink-2)]">{body}</p>
            </div>
          ))}
        </div>
      </section>

      {/* Screenshots grid */}
      <section className="mt-16 border-t border-[var(--ludex-line)] pt-12">
        <div className="flex flex-wrap items-end justify-between gap-4">
          <div>
            <p className="font-mono text-xs uppercase tracking-[0.12em] text-[var(--ludex-ink-3)]">
              Shipped screens
            </p>
            <h2 className="mt-2 text-2xl font-semibold tracking-[-0.02em] text-[var(--ludex-ink)] sm:text-3xl">
              Captured directly from the demo build.
            </h2>
          </div>
          <Link
            href="/docs/development/screenshots"
            className="text-sm text-[var(--ludex-ink-2)] underline underline-offset-4 hover:text-[var(--ludex-ink)]"
          >
            Full screenshots gallery
          </Link>
        </div>

        <div className="mt-8 grid gap-5 md:grid-cols-2">
          {GALLERY.map((shot) => (
            <figure
              key={shot.title}
              className="overflow-hidden rounded-md border border-[var(--ludex-line)] bg-[var(--ludex-surface)]"
            >
              <img src={withBase(shot.src)} alt={shot.alt} className="w-full border-b border-[var(--ludex-line)]" />
              <figcaption className="p-4">
                <p className="text-sm font-semibold text-[var(--ludex-ink)]">{shot.title}</p>
                <p className="mt-1 text-xs leading-relaxed text-[var(--ludex-ink-2)]">
                  {shot.caption}
                </p>
              </figcaption>
            </figure>
          ))}
        </div>
      </section>

      {/* Quick Start: Desktop + Local mode */}
      <section className="mt-16 border-t border-[var(--ludex-line)] pt-12">
        <div className="grid gap-6 lg:grid-cols-3">
          <div className="rounded-md border border-[var(--ludex-line)] bg-[var(--ludex-surface)] p-5">
            <p className="font-mono text-xs uppercase tracking-[0.12em] text-[var(--ludex-ink-3)]">
              01 · Self-hosted server
            </p>
            <h3 className="mt-2 text-base font-semibold text-[var(--ludex-ink)]">
              Docker or bare binary
            </h3>
            <p className="mt-2 text-sm leading-relaxed text-[var(--ludex-ink-2)]">
              Run <code className="font-mono text-xs">ghcr.io/cleeryy/ludex:v{LUDEX_VERSION}</code>{" "}
              with SQLite or PostgreSQL and local disk or S3 storage.
            </p>
            <Link
              href="/docs/deployment/docker-and-proxy"
              className="mt-4 inline-flex items-center gap-1.5 text-sm font-medium text-[var(--ludex-signal)] hover:underline"
            >
              Deployment guide <ArrowRight className="size-3.5" aria-hidden="true" />
            </Link>
          </div>

          <div className="rounded-md border border-[var(--ludex-line)] bg-[var(--ludex-surface)] p-5">
            <p className="font-mono text-xs uppercase tracking-[0.12em] text-[var(--ludex-ink-3)]">
              02 · Desktop app
            </p>
            <h3 className="mt-2 text-base font-semibold text-[var(--ludex-ink)]">
              Windows & Linux installers
            </h3>
            <p className="mt-2 text-sm leading-relaxed text-[var(--ludex-ink-2)]">
              Per-user NSIS installer on Windows, AppImage and <code className="font-mono text-xs">.deb</code>{" "}
              on Linux. Signed with minisign for verified in-app updates.
            </p>
            <Link
              href="/docs/getting-started/desktop-install"
              className="mt-4 inline-flex items-center gap-1.5 text-sm font-medium text-[var(--ludex-signal)] hover:underline"
            >
              Desktop install guide <ArrowRight className="size-3.5" aria-hidden="true" />
            </Link>
          </div>

          <div className="rounded-md border border-[var(--ludex-line)] bg-[var(--ludex-surface)] p-5">
            <p className="font-mono text-xs uppercase tracking-[0.12em] text-[var(--ludex-ink-3)]">
              03 · Zero-server mode
            </p>
            <h3 className="mt-2 text-base font-semibold text-[var(--ludex-ink)]">
              Local library
            </h3>
            <p className="mt-2 text-sm leading-relaxed text-[var(--ludex-ink-2)]">
              Point the desktop app at a local games folder. It boots the embedded server on{" "}
              <code className="font-mono text-xs">127.0.0.1</code> with file-manifest transport
              enabled automatically.
            </p>
            <Link
              href="/docs/getting-started/first-library"
              className="mt-4 inline-flex items-center gap-1.5 text-sm font-medium text-[var(--ludex-signal)] hover:underline"
            >
              First library guide <ArrowRight className="size-3.5" aria-hidden="true" />
            </Link>
          </div>
        </div>
      </section>

      {/* Security & What it is not */}
      <section className="mt-16 grid gap-6 border-t border-[var(--ludex-line)] pt-12 lg:grid-cols-2">
        <div className="rounded-md border border-[var(--ludex-line)] bg-[var(--ludex-surface)] p-6">
          <div className="flex items-center gap-2 text-sm font-semibold text-[var(--ludex-ink)]">
            <Lock className="size-4 text-[var(--ludex-signal)]" aria-hidden="true" />
            Security & data ownership
          </div>
          <ul className="mt-4 space-y-2.5 text-sm leading-relaxed text-[var(--ludex-ink-2)]">
            <li>• Your archives stay on your disk or your private S3 bucket — nothing is uploaded.</li>
            <li>• Argon2id hashes, TOTP 2FA, OIDC SSO, revocable device sessions and role-enforced APIs.</li>
            <li>• Full SHA-256 verification before install, path-traversal guards on every file boundary.</li>
            <li>• Non-root Docker image (uid/gid 1000) with zero telemetry and no phone-home.</li>
          </ul>
          <Link
            href="/docs/security/overview"
            className="mt-4 inline-flex items-center gap-1.5 text-sm font-medium text-[var(--ludex-signal)] hover:underline"
          >
            Security model <ArrowRight className="size-3.5" aria-hidden="true" />
          </Link>
        </div>

        <div className="rounded-md border border-[var(--ludex-line)] bg-[var(--ludex-surface)] p-6">
          <h2 className="text-sm font-semibold text-[var(--ludex-ink)]">What Ludex is not</h2>
          <ul className="mt-4 space-y-2.5 text-sm leading-relaxed text-[var(--ludex-ink-2)]">
            <li>• Not a storefront, torrent index or piracy tool — Ludex never distributes games.</li>
            <li>• Not a cloud-streaming host — your PC runs your games locally.</li>
            <li>• Not open-core — every feature ships under AGPL-3.0-or-later with no paywall.</li>
            <li>• Not affiliated with GameVault, Steam, GOG, IGDB or any storefront.</li>
          </ul>
          <Link
            href="/docs/reference/feature-parity"
            className="mt-4 inline-flex items-center gap-1.5 text-sm font-medium text-[var(--ludex-signal)] hover:underline"
          >
            Honest comparison with GameVault <ArrowRight className="size-3.5" aria-hidden="true" />
          </Link>
        </div>
      </section>

      {/* Footer */}
      <footer className="mt-16 flex flex-wrap items-center justify-between gap-4 border-t border-[var(--ludex-line)] pt-8 text-xs text-[var(--ludex-ink-3)]">
        <p>
          Ludex <span className="font-mono ludex-num">v{LUDEX_VERSION}</span> · AGPL-3.0-or-later ·
          Independent clean-room implementation.
        </p>
        <nav aria-label="Footer links" className="flex flex-wrap items-center gap-5">
          <Link href="/docs" className="hover:text-[var(--ludex-ink)]">
            Documentation
          </Link>
          <a href={RELEASES_URL} target="_blank" rel="noreferrer" className="hover:text-[var(--ludex-ink)]">
            Releases
          </a>
          <a href={`${GITHUB_URL}/blob/main/SECURITY.md`} target="_blank" rel="noreferrer" className="hover:text-[var(--ludex-ink)]">
            Security
          </a>
          <a href={`${GITHUB_URL}/blob/main/LICENSE`} target="_blank" rel="noreferrer" className="hover:text-[var(--ludex-ink)]">
            License
          </a>
          <a href={GITHUB_URL} target="_blank" rel="noreferrer" className="hover:text-[var(--ludex-ink)]">
            GitHub
          </a>
        </nav>
      </footer>
    </main>
  );
}
