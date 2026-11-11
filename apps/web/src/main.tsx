import ReactDOM from "react-dom/client";
import "@fontsource-variable/bricolage-grotesque/wdth.css";
import "@fontsource-variable/instrument-sans";
import "@fontsource-variable/jetbrains-mono";
import "./styles/index.css";
import { App } from "./app";
import { bootstrapI18n } from "./lib/i18n";
import { initPwa } from "./lib/pwa";

// The inline boot script in index.html applies the stored theme and locale
// before the first paint. The SettingsProvider re-applies the theme from the
// same storage once React mounts, so HMR and tests see identical state.
async function start() {
  // Load the persisted locale catalog before the first render so there is no
  // flash of English for French users.
  await bootstrapI18n();
  ReactDOM.createRoot(document.getElementById("root")!).render(<App />);
  initPwa();
}

void start();
