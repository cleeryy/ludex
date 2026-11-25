#!/usr/bin/env node
/**
 * Sync canonical Markdown documentation from root `docs/` into
 * `apps/site/content/docs/` for Fumadocs static export (#367).
 *
 * Root `docs/` is the single source of truth. Never hand-edit files under
 * `apps/site/content/docs/`; CI runs this script and fails on any diff.
 */
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { dirname, join, posix, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SITE_DIR = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const ROOT_DIR = resolve(SITE_DIR, "../..");
const DOCS_SRC = join(ROOT_DIR, "docs");
const CONTENT_OUT = join(SITE_DIR, "content", "docs");
const PUBLIC_IMAGES = join(SITE_DIR, "public", "images");
const GITHUB_BLOB = "https://github.com/cleeryy/ludex/blob/main";

const rawBasePath = process.env.BASE_PATH ?? "/ludex";
const BASE_PATH = rawBasePath === "/" ? "" : rawBasePath.replace(/\/+$/, "");

/** Curated docs IA mapped from canonical root `docs/*.md`. */
const SECTIONS = [
  {
    id: "getting-started",
    title: "Getting Started",
    pages: [
      { src: "INSTALL.md", slug: "install", title: "Server Install" },
      { src: "DESKTOP-INSTALL.md", slug: "desktop-install", title: "Desktop App" },
      { src: "LIBRARY.md", slug: "first-library", title: "First Library & Scanning" },
    ],
  },
  {
    id: "deployment",
    title: "Deployment",
    pages: [
      { src: "DEPLOY.md", slug: "docker-and-proxy", title: "Docker, Reverse Proxy & Systemd" },
      { src: "POSTGRES.md", slug: "postgresql", title: "PostgreSQL Backend" },
      { src: "STORAGE.md", slug: "s3-storage", title: "S3 Object Storage" },
      { src: "BACKUP.md", slug: "backups", title: "Backups & Restore" },
      { src: "GITOPS.md", slug: "gitops", title: "GitOps & Edge Updates" },
    ],
  },
  {
    id: "using-ludex",
    title: "Using Ludex",
    pages: [
      { src: "DOWNLOADS.md", slug: "downloads", title: "Downloads & Transports" },
      { src: "CONSOLE-SESSION.md", slug: "console", title: "Console Mode (10-Foot TV)" },
      { src: "SAVES.md", slug: "saves", title: "Cloud Saves" },
      { src: "ACHIEVEMENTS.md", slug: "achievements", title: "Achievements" },
      { src: "EMULATORS.md", slug: "emulators", title: "Emulators & ROMs" },
      { src: "LINUX-GAMING.md", slug: "linux-gaming", title: "Linux Gaming & Proton" },
      { src: "CONTROLLERS.md", slug: "controllers", title: "Controllers" },
      { src: "STREAMING.md", slug: "streaming", title: "Sunshine & Moonlight Streaming" },
      { src: "I18N.md", slug: "i18n", title: "Languages & Localization" },
    ],
  },
  {
    id: "security",
    title: "Security",
    pages: [
      { src: "SECURITY.md", slug: "overview", title: "Security Model & Auth" },
      { src: "OIDC.md", slug: "oidc", title: "OIDC Single Sign-On" },
      { src: "INTEGRATIONS.md", slug: "integrations", title: "API Keys, Webhooks & VirusTotal" },
      { src: "REMOTE-ACCESS.md", slug: "remote-access", title: "Remote Access & VPN" },
    ],
  },
  {
    id: "reference",
    title: "Reference",
    pages: [
      { src: "OPENAPI.md", slug: "openapi", title: "OpenAPI & API Overview" },
      { src: "COMPAT-RULES.md", slug: "compat-rules", title: "Compatibility Rules" },
      { src: "SETTINGS.md", slug: "settings", title: "Settings Tiers" },
      { src: "FEATURE-PARITY.md", slug: "feature-parity", title: "GameVault Feature Parity" },
      { src: "MIGRATION.md", slug: "migration", title: "Migrating from GameVault" },
      { src: "COMPETITIVE-ROADMAP.md", slug: "competitive-roadmap", title: "Competitive Roadmap" },
      { src: "UX-UI-AUDIT-2026-11.md", slug: "ux-ui-audit", title: "UX & UI Audit (2026-11)" },
    ],
  },
  {
    id: "development",
    title: "Development",
    pages: [
      { src: "ARCHITECTURE.md", slug: "architecture", title: "Architecture" },
      { src: "NETWORK-ARCHITECTURE.md", slug: "network-architecture", title: "Network Architecture" },
      { src: "CI.md", slug: "ci", title: "CI & Quality Gates" },
      { src: "RELEASING.md", slug: "releasing", title: "Releasing Ludex" },
      { src: "SCREENSHOTS.md", slug: "screenshots", title: "Screenshots Gallery" },
      { src: "REPO.md", slug: "repo", title: "Repository Settings & Pages" },
    ],
  },
];

/** Landing page + OG images copied from `docs/assets/`. */
const EXTRA_ASSETS = [
  ["docs/assets/banner.svg", "images/banner.svg"],
  ["docs/assets/social-preview.png", "og.png"],
  ["docs/assets/desktop-first-run.png", "images/desktop-first-run.png"],
  [
    "docs/assets/screenshots/design-333/library-speranza-desktop.png",
    "images/screenshots/design-333/library-speranza-desktop.png",
  ],
  [
    "docs/assets/screenshots/design-333/library-bauhaus-desktop.png",
    "images/screenshots/design-333/library-bauhaus-desktop.png",
  ],
  [
    "docs/assets/screenshots/design-333/home-speranza-desktop.png",
    "images/screenshots/design-333/home-speranza-desktop.png",
  ],
  [
    "docs/assets/screenshots/design-333/downloads-speranza-desktop.png",
    "images/screenshots/design-333/downloads-speranza-desktop.png",
  ],
  [
    "docs/assets/screenshots/design-333/admin-health-speranza-desktop.png",
    "images/screenshots/design-333/admin-health-speranza-desktop.png",
  ],
  [
    "docs/assets/screenshots/console/stage-1080.png",
    "images/screenshots/console/stage-1080.png",
  ],
  [
    "docs/assets/screenshots/console/wall-1080.png",
    "images/screenshots/console/wall-1080.png",
  ],
  [
    "docs/assets/screenshots/readme/game-page.png",
    "images/screenshots/readme/game-page.png",
  ],
];

/** Map canonical `docs/<file>` -> `/docs/<section>/<slug>/` route. */
const DOC_ROUTE_MAP = new Map();
for (const section of SECTIONS) {
  for (const page of section.pages) {
    DOC_ROUTE_MAP.set(
      posix.normalize(page.src),
      `/docs/${section.id}/${page.slug}/`,
    );
  }
}

function ensureDir(dir) {
  mkdirSync(dir, { recursive: true });
}

function copyAssetToPublic(absPath, includeBasePath) {
  const relFromAssets = relative(join(DOCS_SRC, "assets"), absPath).replaceAll("\\", "/");
  if (relFromAssets.startsWith("..")) return null;
  const dest = join(PUBLIC_IMAGES, relFromAssets);
  ensureDir(dirname(dest));
  copyFileSync(absPath, dest);
  return includeBasePath ? `${BASE_PATH}/images/${relFromAssets}` : `/images/${relFromAssets}`;
}

function yamlString(value) {
  return JSON.stringify(value.replace(/\s+/g, " ").trim());
}

function extractTitleAndDescription(markdown, fallbackTitle) {
  const lines = markdown.split(/\r?\n/);
  let title = fallbackTitle;
  let bodyStart = 0;

  for (let i = 0; i < lines.length; i++) {
    const trimmed = lines[i].trim();
    if (!trimmed) continue;
    const h1 = trimmed.match(/^#\s+(.+)$/);
    if (h1) {
      title = h1[1].replace(/`([^`]+)`/g, "$1").trim();
      bodyStart = i + 1;
    }
    break;
  }

  let description = "";
  let inFence = false;
  for (let i = bodyStart; i < lines.length; i++) {
    const line = lines[i].trim();
    if (line.startsWith("```")) {
      inFence = !inFence;
      continue;
    }
    if (inFence || !line) continue;
    if (
      line.startsWith("#") ||
      line.startsWith("|") ||
      line.startsWith("!") ||
      line.startsWith("<") ||
      line.startsWith(">") ||
      line.startsWith("- ") ||
      line.startsWith("* ")
    ) {
      continue;
    }
    description = line
      .replace(/\[([^\]]+)\]\([^)]+\)/g, "$1")
      .replace(/[`*_]/g, "")
      .trim();
    if (description.length > 160) {
      description = `${description.slice(0, 157).trimEnd()}…`;
    }
    break;
  }

  const body = lines.slice(bodyStart).join("\n").replace(/^\n+/, "");
  return { title, description, body };
}

function rewriteLinkTarget(target, srcFileAbs, { rawHtml = false } = {}) {
  if (
    !target ||
    target.startsWith("#") ||
    /^[a-z][a-z0-9+.-]*:/i.test(target) ||
    target.startsWith("//")
  ) {
    return target;
  }

  const [rawPath, hash] = target.split("#");
  const suffix = hash ? `#${hash}` : "";
  if (!rawPath) return target;

  const resolvedAbs = resolve(dirname(srcFileAbs), decodeURIComponent(rawPath));
  const relFromDocs = relative(DOCS_SRC, resolvedAbs).replaceAll("\\", "/");
  const relFromRoot = relative(ROOT_DIR, resolvedAbs).replaceAll("\\", "/");

  if (!relFromDocs.startsWith("..")) {
    const mapped = DOC_ROUTE_MAP.get(posix.normalize(relFromDocs));
    if (mapped) {
      return rawHtml ? `${BASE_PATH}${mapped}${suffix}` : `${mapped}${suffix}`;
    }
  }

  if (/\.(png|jpe?g|gif|webp|svg)$/i.test(rawPath) && existsSync(resolvedAbs)) {
    const pub = copyAssetToPublic(resolvedAbs, rawHtml);
    if (pub) return pub;
  }

  if (!relFromRoot.startsWith("..")) {
    return `${GITHUB_BLOB}/${relFromRoot}${suffix}`;
  }
  return target;
}

/**
 * Sanitize standard GFM Markdown for MDX compilation while leaving fenced
 * code blocks and inline code spans untouched.
 */
function sanitizeProseLine(line, srcFileAbs) {
  // Rewrite markdown links and images outside inline code.
  const segments = line.split(/(`+[^`]*`+)/g);
  return segments
    .map((seg, idx) => {
      if (idx % 2 === 1) return seg; // inline code span

      let out = seg;
      // Remove HTML comments completely (fixed-point loop prevents nested bypasses).
      let prev;
      do {
        prev = out;
        out = out.replace(/<!--[\s\S]*?-->/g, "");
      } while (out !== prev);

      // Convert raw HTML <img> / <source> / <picture> / <table> blocks from docs/SCREENSHOTS.md
      // into clean standard Markdown / self-closing JSX.
      out = out.replace(/<img\b([^>]*?)(?<!\/)>/gi, (_, attrs) => {
        const fixed = attrs.replace(/\bsrc=(["'])([^"']+)\1/i, (__, q, val) => {
          return `src=${q}${rewriteLinkTarget(val, srcFileAbs, { rawHtml: true })}${q}`;
        });
        return `<img${fixed} />`;
      });
      out = out.replace(/<source\b([^>]*?)(?<!\/)>/gi, (_, attrs) => {
        const fixed = attrs.replace(/\bsrcset=(["'])([^"']+)\1/i, (__, q, val) => {
          return `srcset=${q}${rewriteLinkTarget(val, srcFileAbs, { rawHtml: true })}${q}`;
        });
        return `<source${fixed} />`;
      });
      out = out.replace(/<br\s*>/gi, "<br />");
      out = out.replace(/<hr\s*>/gi, "<hr />");

      // Rewrite markdown images ![alt](target) and links [text](target)
      out = out.replace(
        /(!?\[[^\]]*\]\()([^)\s]+)((?:\s+"[^"]*")?\))/g,
        (_, prefix, target, suffix) => {
          return `${prefix}${rewriteLinkTarget(target, srcFileAbs)}${suffix}`;
        },
      );

      // Escape autolinks like <https://...> -> [https://...](https://...)
      out = out.replace(/<(https?:\/\/[^>\s]+)>/g, "[$1]($1)");

      // Escape angle brackets used as prose placeholders (e.g. `<tag>`, `<version>`, `< 5%`, `<=1000`)
      // while preserving known valid HTML tags used in docs.
      const ALLOWED_HTML =
        /^<\/?(?:picture|source|img|table|thead|tbody|tr|td|th|sub|sup|code|kbd|br|hr|details|summary|p|a|strong|em|ul|ol|li)\b/i;
      out = out.replace(/<[^>]+>/g, (match) => {
        if (ALLOWED_HTML.test(match)) return match;
        return match.replace(/</g, "&lt;").replace(/>/g, "&gt;");
      });
      // Escape lone `<` followed by digit or space (e.g. `<100ms`, `<= 300 KB`)
      out = out.replace(/<(?=[\d=\s])/g, "&lt;");

      // Escape `{` and `}` as HTML character references so MDX never parses `{DATA}` or `{id}` as JS expressions.
      out = out.replace(/\{/g, "&#123;").replace(/\}/g, "&#125;");

      return out;
    })
    .join("");
}

function transformMarkdown(raw, srcFileAbs, fallbackTitle) {
  const { title, description, body } = extractTitleAndDescription(raw, fallbackTitle);
  const lines = body.split(/\r?\n/);
  const result = [];
  let inFence = false;
  let fenceChar = "";

  for (const line of lines) {
    const fenceMatch = line.match(/^\s*(```+|~~~+)/);
    if (fenceMatch) {
      const marker = fenceMatch[1][0];
      if (!inFence) {
        inFence = true;
        fenceChar = marker;
      } else if (marker === fenceChar) {
        inFence = false;
        fenceChar = "";
      }
      result.push(line);
      continue;
    }
    if (inFence) {
      result.push(line);
    } else {
      result.push(sanitizeProseLine(line, srcFileAbs));
    }
  }

  const frontmatter = [
    "---",
    `title: ${yamlString(title)}`,
    ...(description ? [`description: ${yamlString(description)}`] : []),
    "---",
    "",
  ].join("\n");

  return `${frontmatter}${result.join("\n").trimEnd()}\n`;
}

// Reset generated directories cleanly so removed docs never linger.
rmSync(CONTENT_OUT, { recursive: true, force: true });
rmSync(PUBLIC_IMAGES, { recursive: true, force: true });
ensureDir(CONTENT_OUT);
ensureDir(PUBLIC_IMAGES);

// Copy static landing + OG assets.
for (const [relSrc, relDest] of EXTRA_ASSETS) {
  const absSrc = join(ROOT_DIR, relSrc);
  const absDest = join(SITE_DIR, "public", relDest);
  if (!existsSync(absSrc)) {
    throw new Error(`Missing required site asset: ${relSrc}`);
  }
  ensureDir(dirname(absDest));
  copyFileSync(absSrc, absDest);
}

// Root docs index page.
const indexCards = SECTIONS.map((section) => {
  const links = section.pages
    .map((page) => `- [${page.title}](/docs/${section.id}/${page.slug}/)`)
    .join("\n");
  return `## ${section.title}\n\n${links}`;
}).join("\n\n");

writeFileSync(
  join(CONTENT_OUT, "index.mdx"),
  [
    "---",
    'title: "Documentation"',
    'description: "Guides, deployment manuals, security model and reference docs for self-hosting Ludex."',
    "---",
    "",
    "Welcome to the Ludex documentation. Every page below is generated automatically from the canonical Markdown files in [`docs/`](https://github.com/cleeryy/ludex/tree/main/docs).",
    "",
    indexCards,
    "",
    "## Full API & Tauri Contract",
    "",
    `The full machine-checked endpoint and Tauri command contract lives in [\`docs/CONTRACT.md\`](${GITHUB_BLOB}/docs/CONTRACT.md) alongside the [OpenAPI overview](/docs/reference/openapi/).`,
    "",
  ].join("\n"),
  "utf8",
);

writeFileSync(
  join(CONTENT_OUT, "meta.json"),
  JSON.stringify(
    {
      title: "Ludex Docs",
      pages: ["index", ...SECTIONS.map((section) => section.id)],
    },
    null,
    2,
  ) + "\n",
  "utf8",
);

let syncedCount = 0;
for (const section of SECTIONS) {
  const sectionDir = join(CONTENT_OUT, section.id);
  ensureDir(sectionDir);

  writeFileSync(
    join(sectionDir, "meta.json"),
    JSON.stringify(
      {
        title: section.title,
        pages: section.pages.map((page) => page.slug),
      },
      null,
      2,
    ) + "\n",
    "utf8",
  );

  for (const page of section.pages) {
    const srcAbs = join(DOCS_SRC, page.src);
    if (!existsSync(srcAbs)) {
      throw new Error(`Missing canonical doc: docs/${page.src}`);
    }
    const raw = readFileSync(srcAbs, "utf8");
    const mdx = transformMarkdown(raw, srcAbs, page.title);
    writeFileSync(join(sectionDir, `${page.slug}.mdx`), mdx, "utf8");
    syncedCount += 1;
  }
}

process.stdout.write(
  `sync-docs: generated ${syncedCount} doc page(s) across ${SECTIONS.length} section(s) (basePath=${BASE_PATH || "/"})\n`,
);
