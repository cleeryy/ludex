import { useEffect } from "react";
import ReactDOM from "react-dom/client";
import "@fontsource-variable/bricolage-grotesque/wdth.css";
import "@fontsource-variable/instrument-sans";
import "@fontsource-variable/jetbrains-mono";
import "./styles/index.css";
import { App } from "./app";

function Root() {
  // Inline theme bootstrap: applies the stored theme before the first paint
  // so there is no flash of the wrong surface.
  useEffect(() => {
    const root = document.documentElement;
    const pref = localStorage.getItem("ludex.theme") ?? "system";
    const dark = pref === "dark" || (pref === "system" && !window.matchMedia("(prefers-color-scheme: light)").matches);
    root.dataset.theme = dark ? "dark" : "light";
    const meta = document.querySelector('meta[name="theme-color"]');
    meta?.setAttribute("content", dark ? "#0a0a09" : "#e7e2d4");
  }, []);

  return <App />;
}

ReactDOM.createRoot(document.getElementById("root")!).render(<Root />);
