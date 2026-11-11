// Drives Chrome over the DevTools protocol: opens the Lite edition with a fixture (the dev
// server's ?fixture= shortcut), picks the Kodak Portra 400 look in Simplified Chinese, and saves
// a screenshot. Usage: node scripts/web-screenshot.mjs <chrome> <url> <out.png> [1 for headless]
import { spawn } from "node:child_process";
import { writeFileSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
const [chrome, url, out, headless] = process.argv.slice(2);
const profile = mkdtempSync(join(tmpdir(), "op-shot-"));
const args = ["--remote-debugging-port=9333", `--user-data-dir=${profile}`, "--window-size=1440,900", "--enable-unsafe-webgpu",
  "--no-first-run", "--no-default-browser-check", "--hide-scrollbars", "--force-device-scale-factor=1", "--lang=zh-CN"];
if (headless === "1") args.push("--headless=new");
const proc = spawn(chrome, [...args, "about:blank"], { stdio: "ignore" });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let targets;
for (let i = 0; i < 50; i++) { try { targets = await (await fetch("http://127.0.0.1:9333/json")).json(); break; } catch { await sleep(200); } }
const page = targets.find((t) => t.type === "page");
const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((r) => (ws.onopen = r));
let id = 0; const pending = new Map();
ws.onmessage = (e) => { const m = JSON.parse(e.data); if (m.id && pending.has(m.id)) { pending.get(m.id)(m); pending.delete(m.id); } };
const send = (method, params = {}) => new Promise((r) => { const n = ++id; pending.set(n, r); ws.send(JSON.stringify({ id: n, method, params })); });
const evaluate = async (expression) => (await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true })).result?.result?.value;
await send("Emulation.setDeviceMetricsOverride", { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false });
await send("Page.navigate", { url: url.replace(/\?.*/, "") });
await sleep(1500);
await evaluate(`localStorage.setItem("openpixels.language.v1", "zh-CN"); Object.keys(localStorage).join(",")`);
await send("Page.navigate", { url });
const pick = (match) => `(() => { const s = [...document.querySelectorAll("select")].find(s => [...s.options].some(o => o.text.includes(${JSON.stringify(match)})));
  if (!s) return "none"; const o = [...s.options].find(o => o.text.includes(${JSON.stringify(match)}));
  Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value").set.call(s, o.value); s.dispatchEvent(new Event("change", { bubbles: true })); return o.text; })()`;
for (let i = 0; i < 60; i++) { if (await evaluate(`!!document.querySelector("canvas") && [...document.querySelectorAll("select")].some(s => [...s.options].some(o => o.text.includes("Portra 400")))`)) break; await sleep(500); }
await sleep(4000);
console.log("language", await evaluate(`document.documentElement.lang + " " + document.querySelector("select.language")?.value`));
console.log("look", await evaluate(pick("Portra 400")));
await sleep(5000);
console.log("errors", await evaluate(`document.querySelector(".error, [role=alert]")?.textContent ?? ""`));
const shot = await send("Page.captureScreenshot", { format: "png" });
writeFileSync(out, Buffer.from(shot.result.data, "base64"));
ws.close(); proc.kill();
