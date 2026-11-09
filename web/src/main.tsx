import { createRoot } from "react-dom/client";
import { App } from "./App";
import "./styles.css";

// No StrictMode: its double-run effects would transfer the canvas to the worker twice.
createRoot(document.getElementById("root")!).render(<App />);
