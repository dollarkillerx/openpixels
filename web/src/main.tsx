import { createRoot } from "react-dom/client";
import { initI18n } from "./i18n";
import "./styles.css";

// The language loads first: the app's modules build their labels when they are imported.
await initI18n();
const { App } = await import("./App");

// No StrictMode: its double-run effects would transfer the canvas to the worker twice.
createRoot(document.getElementById("root")!).render(<App />);
