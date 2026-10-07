import { StrictMode, lazy, Suspense } from "react";
import { createRoot } from "react-dom/client";
import "./styles.css";
// One file per theme; the order here is the cascade order.
import "./themes/night.css";
import "./themes/day.css";
import "./themes/werk.css";
import "./themes/gemma.css";
import "./themes/migu.css";
import "./themes/nerv.css";
import "./themes/future.css";
import "./themes/overlay.css";

// Lazy entries: the main window must not load the overlay's modules and the
// overlay window must not load the whole app — in dev that halves the module
// graph each window fetches, and the overlay starts with almost nothing.
const App = lazy(() => import("./App"));
const Overlay = lazy(() => import("./pages/Overlay"));

const overlay = new URLSearchParams(window.location.search).get("overlay") === "1";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Suspense fallback={null}>{overlay ? <Overlay /> : <App />}</Suspense>
  </StrictMode>,
);
