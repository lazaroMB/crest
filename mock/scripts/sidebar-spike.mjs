// Runs the frozen browser checks of spikes S06 (criteria 2-4) and S05
// (CREST-SIDEBAR) in headless Chrome over the DevTools protocol, and saves a
// screenshot of each state to spikes/evidence/. Needs the dev server:
//
//   npx vite --port 5299 &
//   PORT=5299 node scripts/sidebar-spike.mjs

import { spawn } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";

const CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const PORT = process.env.PORT ?? "5199";
const DEBUG = 9334;
const OUT = "spikes/evidence";

const chrome = spawn(CHROME, [
  "--headless=new",
  "--disable-gpu",
  "--hide-scrollbars",
  `--remote-debugging-port=${DEBUG}`,
  "--user-data-dir=/tmp/crest-sidebar-chrome",
  "about:blank",
]);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function pageSocket() {
  for (let i = 0; i < 50; i++) {
    try {
      const page = (await (await fetch(`http://127.0.0.1:${DEBUG}/json`)).json()).find((t) => t.type === "page");
      if (page) return page.webSocketDebuggerUrl;
    } catch {}
    await sleep(100);
  }
  throw new Error("Chrome did not start");
}

const ws = new WebSocket(await pageSocket());
await new Promise((r) => ws.addEventListener("open", r));
let nextId = 1;
const pending = new Map();
const errors = [];
ws.addEventListener("message", ({ data }) => {
  const msg = JSON.parse(data);
  if (msg.id) pending.get(msg.id)?.(msg.result);
  if (msg.method === "Runtime.exceptionThrown") errors.push(msg.params.exceptionDetails.exception?.description);
});
const send = (method, params = {}) =>
  new Promise((resolve) => {
    const id = nextId++;
    pending.set(id, resolve);
    ws.send(JSON.stringify({ id, method, params }));
  });
const evaluate = async (expression) => (await send("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true })).result.value;

async function open(query, width = 1280) {
  await send("Emulation.setDeviceMetricsOverride", { width, height: 800, deviceScaleFactor: 1, mobile: false });
  await send("Page.navigate", { url: `http://localhost:${PORT}/${query}` });
  await sleep(1500);
  await evaluate(`document.querySelector('.mockbar') && [...document.querySelectorAll('.mockbar__button')].find(b => b.textContent.startsWith('hide'))?.click()`);
  await sleep(200);
}

async function shot(name) {
  mkdirSync(OUT, { recursive: true });
  const { data } = await send("Page.captureScreenshot", { format: "png" });
  writeFileSync(`${OUT}/${name}.png`, Buffer.from(data, "base64"));
}

const widths = () =>
  evaluate(`(() => {
    const w = (sel) => Math.round(document.querySelector(sel)?.getBoundingClientRect().width ?? 0);
    return { window: w('.window'), sidebar: w('aside.chats'), session: w('.session'), timeline: w('.timeline') || w('.session') };
  })()`);

const key = async (keyName, { ctrl = false } = {}) => {
  const code = keyName.length === 1 ? `Key${keyName.toUpperCase()}` : keyName;
  const base = { key: keyName, code, modifiers: ctrl ? 2 : 0, windowsVirtualKeyCode: keyName.toUpperCase().charCodeAt(0) };
  await send("Input.dispatchKeyEvent", { type: "keyDown", ...base });
  await send("Input.dispatchKeyEvent", { type: "keyUp", ...base });
  await sleep(150);
};

const results = {};
const check = (spike, id, ok, detail = "") => {
  (results[spike] ??= []).push(ok);
  console.log(`${spike}.${id} ${ok ? "met" : "not-met"}  (${detail})`);
};

await send("Runtime.enable");
await send("Page.enable");

// S06: the compact mode control.
await open("?chat=c-bugfix");
const before = await evaluate(`(() => {
  const row = document.querySelector('.prompt__row');
  const kids = [...row.children];
  const plus = row.querySelector('.prompt__plus');
  return { plusFirst: kids.indexOf(plus) === 0 && kids.indexOf(plus) < kids.indexOf(row.querySelector('.prompt__text')) };
})()`);
await evaluate(`document.querySelector('.prompt__plus').dispatchEvent(new MouseEvent('mousedown', { bubbles: true }))`);
await sleep(300);
const menu = await evaluate(`[...document.querySelectorAll('.mode-menu__label')].map(e => e.textContent)`);
await shot("S06-mode-menu");
check("S06", 2, before.plusFirst && ["Chat", "Research", "Deep research"].every((l) => menu?.includes(l)), `button first: ${before.plusFirst}; menu: ${menu?.join(", ")}`);

await evaluate(`[...document.querySelectorAll('.mode-menu__row')].find(r => r.textContent.includes('Deep research')).dispatchEvent(new MouseEvent('mousedown', { bubbles: true }))`);
await sleep(300);
const chip = await evaluate(`document.querySelector('.prompt__row .prompt__chip')?.textContent ?? ''`);
await shot("S06-deep-chip");
check("S06", 3, chip.includes("Deep research"), `chip: "${chip}"`);

const cost = await evaluate(`(() => {
  const r = (el) => el.getBoundingClientRect();
  const plus = r(document.querySelector('.prompt__plus'));
  const chip = r(document.querySelector('.prompt__chip'));
  return Math.round(chip.right - plus.left);
})()`);
check("S06", 4, cost <= 160, `${cost} px for button and chip`);

// S05: the three widths.
await open("?chat=c-bugfix&sidebar=full");
let w = await widths();
await shot("S05-full-1280");
check("S05", 1, w.sidebar === 240 && w.session / w.window >= 0.75, `sidebar ${w.sidebar} px, session ${w.session} of ${w.window} px (${Math.round((100 * w.session) / w.window)}%)`);

await open("?chat=c-bugfix&sidebar=strip");
w = await widths();
const stripButtons = await evaluate(`['.strip-new', '.strip-search', '.strip-projects'].every(s => document.querySelector('aside.chats ' + s))`);
await shot("S05-strip-1280");
check("S05", 2, w.sidebar <= 56 && stripButtons, `strip ${w.sidebar} px, new/search/projects: ${stripButtons}`);

await open("?chat=c-bugfix&sidebar=full");
const seen = [];
const sidebarState = () => evaluate(`document.querySelector('aside.chats')?.dataset.sidebar ?? (document.querySelector('.sidebar-reveal') ? 'hidden' : '?')`);
seen.push(await sidebarState());
for (let i = 0; i < 3; i++) {
  await key("g", { ctrl: true });
  await key("s");
  await sleep(200);
  seen.push(await sidebarState());
  if (seen.at(-1) === "hidden") {
    w = await widths();
    await shot("S05-hidden-1280");
  }
}
const hiddenWidth = w.sidebar;
check("S05", 3, seen.join(">") === "full>strip>hidden>full" && hiddenWidth === 0, `cycle ${seen.join(" > ")}, hidden sidebar ${hiddenWidth} px`);

await open("?chat=c-bugfix&sidebar=strip", 1024);
w = await widths();
await shot("S05-strip-1024");
check("S05", 4, w.session / w.window >= 0.9, `session ${w.session} of ${w.window} px (${Math.round((100 * w.session) / w.window)}%)`);

// Extra evidence, not criteria: the full sidebar with a standalone chat menu and a project menu.
await open("?chat=c-solo-1");
await evaluate(`document.querySelector('.chat-row--open .chat-row__more')?.click()`);
await sleep(200);
await evaluate(`[...document.querySelectorAll('.chat-menu__row')].find(b => b.textContent.startsWith('Add to project'))?.click()`);
await sleep(200);
await shot("extra-promote-menu");
await open("?chat=c-bugfix");
await evaluate(`document.querySelector('.project__row .project__action:last-child')?.click()`);
await sleep(200);
await shot("extra-project-menu");
await open("?chat=c-bugfix");
await evaluate(`(() => { const t = document.querySelector('.prompt__text'); t.focus(); t.value = '/'; t.dispatchEvent(new Event('input')); })()`);
await sleep(200);
await shot("extra-palette-groups");

if (errors.length) console.log("page errors:\n" + errors.join("\n"));
let failed = 0;
for (const [spike, oks] of Object.entries(results)) {
  const met = oks.filter(Boolean).length;
  console.log(`${spike}: ${met} of ${oks.length}${spike === "S06" ? " (criteria 2-4; 1 runs in organization-spike.mjs)" : ""}`);
  if (met !== oks.length) failed += 1;
}
ws.close();
chrome.kill();
process.exit(failed ? 1 : 0);
