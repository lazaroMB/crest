// Screenshots of the mock in headless Chrome, driven over the DevTools protocol.
//
//   node scripts/shot.mjs <name> "<query>" [wait ms] [keys]
//   node scripts/shot.mjs approval "?chat=new&prompt=rename add to sum" 4000
//   node scripts/shot.mjs palette "?chat=new" 1500 "/re"
//   node scripts/shot.mjs tree "" 1500 "js:[...document.querySelectorAll('.mockbar__button')].find(b => b.textContent === 'tree').click()"
//
// The last argument is typed into the prompt, or run in the page when it starts with "js:".
// Writes shots/<name>.png and prints the page's console errors. Needs `npm run dev`.

import { spawn } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";

const CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const PORT = 9333;
const [name = "shot", query = "", wait = "2000", keys = ""] = process.argv.slice(2);

const chrome = spawn(CHROME, [
  "--headless=new",
  "--disable-gpu",
  "--hide-scrollbars",
  "--window-size=1280,800",
  `--remote-debugging-port=${PORT}`,
  `--user-data-dir=/tmp/crest-mock-chrome`,
  "about:blank",
]);

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function pageSocket() {
  for (let i = 0; i < 50; i++) {
    try {
      const targets = await (await fetch(`http://127.0.0.1:${PORT}/json`)).json();
      const page = targets.find((t) => t.type === "page");
      if (page) return page.webSocketDebuggerUrl;
    } catch {}
    await sleep(100);
  }
  throw new Error("Chrome did not start");
}

const ws = new WebSocket(await pageSocket());
await new Promise((resolve) => ws.addEventListener("open", resolve));

let nextId = 1;
const pending = new Map();
const errors = [];
ws.addEventListener("message", ({ data }) => {
  const msg = JSON.parse(data);
  if (msg.id) pending.get(msg.id)?.(msg.result);
  if (msg.method === "Runtime.exceptionThrown") errors.push(msg.params.exceptionDetails.exception?.description);
  if (msg.method === "Runtime.consoleAPICalled" && msg.params.type === "error")
    errors.push(msg.params.args.map((a) => a.value ?? a.description).join(" "));
});
const send = (method, params = {}) =>
  new Promise((resolve) => {
    const id = nextId++;
    pending.set(id, resolve);
    ws.send(JSON.stringify({ id, method, params }));
  });

await send("Runtime.enable");
await send("Page.enable");
await send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 800, deviceScaleFactor: 1, mobile: false });
await send("Page.navigate", { url: `http://localhost:5199/${query}` });
await sleep(Number(wait));

if (keys.startsWith("js:")) {
  await send("Runtime.evaluate", { expression: keys.slice(3) });
  await sleep(300);
} else if (keys) {
  await send("Input.insertText", { text: keys });
  await sleep(300);
}

const { data } = await send("Page.captureScreenshot", { format: "png" });
mkdirSync("shots", { recursive: true });
writeFileSync(`shots/${name}.png`, Buffer.from(data, "base64"));
console.log(`shots/${name}.png`);
if (errors.length) console.log("errors:\n" + errors.join("\n"));

ws.close();
chrome.kill();
process.exit(0);
