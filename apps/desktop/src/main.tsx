// Entry point of the desktop UI: loads fonts and styles and mounts the React tree in the
// webview (or in a browser tab, where the in-memory mock stands in for the daemon).
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

// bundled locally: no network at runtime, and the CSP only allows 'self'
import "@fontsource-variable/bricolage-grotesque/opsz.css";
import "@fontsource/ibm-plex-sans/400.css";
import "@fontsource/ibm-plex-sans/500.css";
import "@fontsource/ibm-plex-sans/600.css";
import "@fontsource/ibm-plex-mono/400.css";
import "@fontsource/ibm-plex-mono/500.css";
import "./styles/tokens.css";
import "./styles/app.css";

import { App } from "./App";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
