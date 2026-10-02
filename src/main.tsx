import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import "./styles.css";
// One file per theme; the order here is the cascade order.
import "./themes/night.css";
import "./themes/day.css";
import "./themes/werk.css";
import "./themes/gemma.css";
import "./themes/migu.css";
import "./themes/nerv.css";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
