import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { api } from "./lib/api";
import { run, useUI } from "./lib/store";
import * as collab from "./lib/collab";
// Self-hosted type (SIL OFL 1.1): Inter for the interface, JetBrains Mono for timecode and data.
import "@fontsource-variable/inter";
import "@fontsource-variable/jetbrains-mono";
import "./styles.css";

// Dev-only handle used by the automated UI walkthrough (scripts/ui-walkthrough).
if (import.meta.env.DEV) Object.assign(window, { __pirosman: { api, run, useUI, collab } });

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
