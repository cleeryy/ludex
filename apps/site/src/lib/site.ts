import type { BaseLayoutProps } from "fumadocs-ui/layouts/shared";

export const BASE_PATH = (process.env.NEXT_PUBLIC_BASE_PATH ?? "/ludex").replace(/\/+$/, "");
export const SITE_URL = (
  process.env.NEXT_PUBLIC_SITE_URL ?? "https://cleeryy.github.io/ludex"
).replace(/\/+$/, "");
export const LUDEX_VERSION = process.env.NEXT_PUBLIC_LUDEX_VERSION ?? "0.5.2";

export const GITHUB_REPO = "cleeryy/ludex";
export const GITHUB_URL = `https://github.com/${GITHUB_REPO}`;
export const RELEASES_URL = `${GITHUB_URL}/releases`;

export function withBase(path: string): string {
  const normalized = path.startsWith("/") ? path : `/${path}`;
  return `${BASE_PATH}${normalized}`;
}

export function baseOptions(): BaseLayoutProps {
  return {
    nav: {
      title: "Ludex",
      url: "/",
    },
    githubUrl: GITHUB_URL,
    links: [
      {
        text: "Documentation",
        url: "/docs",
        active: "nested-url",
      },
      {
        text: "Quick Start",
        url: "/docs/getting-started/install",
      },
      {
        text: "Releases",
        url: RELEASES_URL,
        external: true,
      },
    ],
  };
}
