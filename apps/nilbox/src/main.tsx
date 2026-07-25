import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./i18n/index";
import "./styles/global.css";
import { applyMonoFontSize, applyMonoLigature, ensureMonoFontLoaded } from "./lib/monoFont";

// Reflect the mono font preferences before first paint, and warm the bundled
// D2Coding faces so xterm measures the real glyph width when it first opens.
applyMonoLigature();
applyMonoFontSize();
void ensureMonoFontLoaded();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
