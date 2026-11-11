import type { ReactNode } from "react";
import { cn } from "@/lib/cn";
import { useSettings } from "@/state/settings";
import { cssVarsFor, builtinTheme } from "@/theme/registry";
import { deriveTheme } from "@/theme/derive";
import type { BuiltinThemeId } from "@/theme/types";

/**
 * Scopes a dark stage to the theme registry (#140). Game detail, the home hero
 * and public share pages sit on artwork in every theme; they used to carry a
 * hand-written `.force-dark` token block. The values now come from the base
 * palette, with the user's accent layered on so stage controls still match.
 */
export function ThemeScope({
  theme = "midnight",
  as: Tag = "div",
  className,
  children,
}: {
  theme?: BuiltinThemeId;
  as?: "div" | "section";
  className?: string;
  children: ReactNode;
}) {
  const { accent } = useSettings();
  const base = builtinTheme(theme);
  const tokens = deriveTheme(base, accent, null).tokens;
  return (
    <Tag
      data-theme={theme}
      data-theme-scope={theme}
      className={cn("text-ink", className)}
      style={cssVarsFor(tokens)}
    >
      {children}
    </Tag>
  );
}
