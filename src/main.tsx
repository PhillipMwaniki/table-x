import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./styles.css";

/**
 * Suppress the webview's own context menu, app-wide.
 *
 * "Reload", "Save as", "Print", "Inspect" are browser affordances that mean
 * nothing in a database client — and "Reload" is actively harmful, offering to
 * discard editor contents that live only in memory. The app supplies its own
 * menu wherever a right-click should do something; everywhere else it does
 * nothing, which is how a native application behaves.
 *
 * Registered here rather than in a component so it covers the document from the
 * first paint, including anything rendered outside the React root.
 */
window.addEventListener("contextmenu", (e) => e.preventDefault());

const root = document.getElementById("root");
if (!root) throw new Error("#root not found");

ReactDOM.createRoot(root).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
