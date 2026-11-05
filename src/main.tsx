import "@fontsource-variable/onest/wght.css";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { applyTheme, loadTheme } from "./prefs";
import "@mobyuz/moby-ui/styles.css";
import "@mobyuz/moby-ui/mood.css";
import "./styles.css";
import "./app.css";

applyTheme(loadTheme());

const root = document.getElementById("root");
if (!root) throw new Error("Missing #root element");

createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
