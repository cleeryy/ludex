import type { Metadata } from "next";
import type { ReactNode } from "react";
import { SiteProvider } from "@/components/provider";
import { LUDEX_VERSION, SITE_URL, withBase } from "@/lib/site";
import "./global.css";

export const metadata: Metadata = {
  metadataBase: new URL(`${SITE_URL}/`),
  title: {
    default: "Ludex — The game library you host yourself",
    template: "%s · Ludex",
  },
  description:
    `Self-hosted, DRM-free game library (v${LUDEX_VERSION}). One Rust binary with SQLite by default, Tauri 2 desktop app, resilient install transports, cloud saves and zero telemetry.`,
  openGraph: {
    title: "Ludex — The game library you host yourself",
    description:
      "Index the DRM-free game archives you already own, enrich them with metadata, and install, launch and track games from one Rust binary and a Tauri 2 desktop app.",
    url: `${SITE_URL}/`,
    siteName: "Ludex",
    images: [
      {
        url: withBase("/og.png"),
        width: 1280,
        height: 640,
        alt: "Ludex — self-hosted DRM-free game library",
      },
    ],
    type: "website",
  },
  twitter: {
    card: "summary_large_image",
    title: "Ludex — The game library you host yourself",
    description:
      "Self-hosted, DRM-free game library. One Rust binary, Tauri 2 desktop app, no vendor account, no telemetry.",
    images: [withBase("/og.png")],
  },
  alternates: {
    canonical: `${SITE_URL}/`,
  },
};

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html lang="en" suppressHydrationWarning>
      <body className="flex min-h-screen flex-col antialiased">
        <SiteProvider>{children}</SiteProvider>
      </body>
    </html>
  );
}
