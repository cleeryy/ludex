import { useEffect } from "react";
import ReactDOM from "react-dom/client";
import "@fontsource-variable/bricolage-grotesque/wdth.css";
import "@fontsource-variable/instrument-sans";
import "@fontsource-variable/jetbrains-mono";
import "./styles/index.css";
import { App } from "./app";
import { bootstrapI18n } from "./lib/i18n";
import { initPwa } from "./lib/pwa";

function Root() {
  // The inline boot script in index.html applies the stored theme and locale
  // before the first paint. Re-apply here so HMR and tests see the same state.
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

async function start() {
  // Load the persisted locale catalog before the first render so there is no
  // flash of English for French users.
  await bootstrapI18n();
  ReactDOM.createRoot(document.getElementById("root")!).render(<Root />);
  initPwa();
}

void start();
