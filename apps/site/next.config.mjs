import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { createMDX } from "fumadocs-mdx/next";

const withMDX = createMDX();

const rootPkg = JSON.parse(
  readFileSync(resolve(import.meta.dirname, "../../package.json"), "utf8"),
);

const rawBasePath = process.env.BASE_PATH ?? "/ludex";
const basePath =
  rawBasePath === "/" ? "" : rawBasePath.replace(/\/+$/, "");

const siteUrl = (
  process.env.SITE_URL ??
  (basePath
    ? `https://cleeryy.github.io${basePath}`
    : "https://cleeryy.github.io/ludex")
).replace(/\/+$/, "");

/** @type {import('next').NextConfig} */
const config = {
  output: "export",
  trailingSlash: true,
  basePath,
  images: {
    unoptimized: true,
  },
  env: {
    NEXT_PUBLIC_BASE_PATH: basePath,
    NEXT_PUBLIC_SITE_URL: siteUrl,
    NEXT_PUBLIC_LUDEX_VERSION:
      process.env.LUDEX_VERSION ?? rootPkg.version ?? "0.5.2",
  },
};

export default withMDX(config);
