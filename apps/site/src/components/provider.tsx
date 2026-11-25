"use client";

import type { ReactNode } from "react";
import { RootProvider } from "fumadocs-ui/provider/next";
import { BASE_PATH } from "@/lib/site";

export function SiteProvider({ children }: { children: ReactNode }) {
  return (
    <RootProvider
      search={{
        options: {
          type: "static",
          api: `${BASE_PATH}/api/search.json`,
        },
      }}
      theme={{
        defaultTheme: "dark",
        enableSystem: true,
      }}
    >
      {children}
    </RootProvider>
  );
}
