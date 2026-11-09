import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from "react";

export type ThemePref = "dark" | "light" | "system";
export type LibraryView = "grid" | "list";

const THEME_KEY = "ludex.theme";
const VIEW_KEY = "ludex.libraryView";

interface SettingsValue {
  theme: ThemePref;
  resolved: "dark" | "light";
  setTheme: (theme: ThemePref) => void;
  view: LibraryView;
  setView: (view: LibraryView) => void;
}

const SettingsContext = createContext<SettingsValue | null>(null);

function systemTheme(): "dark" | "light" {
  return window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
}

function applyTheme(pref: ThemePref): "dark" | "light" {
  const resolved = pref === "system" ? systemTheme() : pref;
  document.documentElement.dataset.theme = resolved;
  const meta = document.querySelector('meta[name="theme-color"]');
  meta?.setAttribute("content", resolved === "dark" ? "#0a0a09" : "#e7e2d4");
  return resolved;
}

export function SettingsProvider({ children }: { children: ReactNode }) {
  const [theme, setThemeState] = useState<ThemePref>(() => (localStorage.getItem(THEME_KEY) as ThemePref) || "system");
  const [resolved, setResolved] = useState<"dark" | "light">(() => applyTheme((localStorage.getItem(THEME_KEY) as ThemePref) || "system"));
  const [view, setViewState] = useState<LibraryView>(() => (localStorage.getItem(VIEW_KEY) as LibraryView) || "grid");

  const setTheme = useCallback((next: ThemePref) => {
    setThemeState(next);
    localStorage.setItem(THEME_KEY, next);
    setResolved(applyTheme(next));
  }, []);

  const setView = useCallback((next: LibraryView) => {
    setViewState(next);
    localStorage.setItem(VIEW_KEY, next);
  }, []);

  useEffect(() => {
    if (theme !== "system") return;
    const media = window.matchMedia("(prefers-color-scheme: light)");
    const onChange = () => setResolved(applyTheme("system"));
    media.addEventListener("change", onChange);
    return () => media.removeEventListener("change", onChange);
  }, [theme]);

  const value = useMemo(
    () => ({ theme, resolved, setTheme, view, setView }),
    [theme, resolved, setTheme, view, setView],
  );

  return <SettingsContext.Provider value={value}>{children}</SettingsContext.Provider>;
}

export function useSettings(): SettingsValue {
  const value = useContext(SettingsContext);
  if (!value) throw new Error("useSettings must be used inside SettingsProvider");
  return value;
}
