import type { MetadataRoute } from "next";
import { source } from "@/lib/source";
import { SITE_URL } from "@/lib/site";

export const dynamic = "force-static";

export default function sitemap(): MetadataRoute.Sitemap {
  const now = new Date();
  const docPages = source.getPages().map((page) => ({
    url: `${SITE_URL}${page.url.endsWith("/") ? page.url : `${page.url}/`}`,
    lastModified: now,
  }));

  return [
    {
      url: `${SITE_URL}/`,
      lastModified: now,
    },
    ...docPages,
  ];
}
