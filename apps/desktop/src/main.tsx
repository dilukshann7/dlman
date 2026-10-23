import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles/globals.css";
// Initializes i18next and applies the saved language/direction/font before render.
import "./i18n";
import { rememberWindowState } from "./lib/window-state";

// Initialize once per webview, outside React StrictMode's effect replay.
if (window.__TAURI_INTERNALS__) {
  rememberWindowState()
    .then((cleanup) => window.addEventListener('pagehide', cleanup, { once: true }))
    .catch((error) => console.error('Failed to initialize window state:', error));
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
