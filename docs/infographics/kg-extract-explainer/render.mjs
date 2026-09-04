#!/usr/bin/env node
// 1:1 long-page renderer for the kg-extract explainer.
//
// Drives headless Chrome over raw CDP (node's built-in WebSocket, zero npm
// deps), captures the page as full-width fixed-height slices from y=0 in
// order, and writes each slice as PNG. Stitching + assertions live in
// stitch.py; this file only captures.
//
// Anti-cache: every run launches a throwaway user-data-dir, so nothing is
// ever served from cache; the capture waits for document.fonts.ready plus
// two animation frames before shooting.
//
// Usage: node render.mjs <abs-path-to-index.html> <abs-out-dir> [dpr]
// Env:   CHROME_BIN overrides the Chrome executable (default: macOS path).
import { execFile } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const htmlPath = process.argv[2];
const outDir = process.argv[3];
const dpr = Number(process.argv[4] || 2);
if (!htmlPath || !outDir || !Number.isFinite(dpr) || dpr <= 0) {
  console.error("usage: node render.mjs <index.html> <out-dir> [dpr]");
  process.exit(2);
}

const CHROME = process.env.CHROME_BIN ||
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const PAGE_W = 1200;      // CSS px, matches the page's own width
const SLICE_H = 900;      // CSS px per slice, fixed height, from y=0 in order

function fail(msg) {
  console.error("RENDER FATAL: " + msg);
  process.exit(3);
}

// -- launch chrome with a throwaway profile, grab the devtools ws url -------
const profile = mkdtempSync(join(tmpdir(), "ig-kgx-chrome."));
const child = execFile(CHROME, [
  "--headless=new",
  "--remote-debugging-port=0",
  "--user-data-dir=" + profile,
  "--no-first-run",
  "--no-default-browser-check",
  "--hide-scrollbars",
  "--force-color-profile=srgb",
  "--disable-gpu",
  "about:blank",
], { maxBuffer: 1 << 26 });

let wsUrl = null;
const wsUrlPromise = new Promise((resolve) => {
  function scan(chunk) {
    const m = /DevTools listening on (ws:\/\/\S+)/.exec(chunk);
    if (m) { wsUrl = m[1]; resolve(wsUrl); }
  }
  child.stderr.on("data", scan);
  child.stdout.on("data", scan);
});
const timer = setTimeout(() => fail("chrome never exposed a devtools websocket"), 30000);
await wsUrlPromise;
clearTimeout(timer);

// -- minimal cdp client over the builtin websocket ---------------------------
let seq = 0;
const pending = new Map();
const events = [];
const ws = new WebSocket(wsUrl);
await new Promise((res, rej) => { ws.onopen = res; ws.onerror = (e) => rej(e.message); });
ws.onmessage = (ev) => {
  const msg = JSON.parse(ev.data);
  if (msg.id && pending.has(msg.id)) {
    const { res, rej } = pending.get(msg.id);
    pending.delete(msg.id);
    msg.error ? rej(new Error(msg.error.message)) : res(msg.result);
  } else if (msg.method) {
    events.push(msg);
  }
};
function send(method, params = {}, sessionId) {
  const id = ++seq;
  return new Promise((res, rej) => {
    pending.set(id, { res, rej });
    ws.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }));
  });
}
async function waitFor(predicate, timeoutMs, what) {
  const t0 = Date.now();
  for (;;) {
    const hit = events.findIndex(predicate);
    if (hit >= 0) { events.splice(hit, 1); return; }
    if (Date.now() - t0 > timeoutMs) fail("timeout waiting for " + what);
    await new Promise((r) => setTimeout(r, 25));
  }
}

// -- open the page flat ------------------------------------------------------
const { targetId } = await send("Target.createTarget", { url: "about:blank" });
const { sessionId } = await send("Target.attachToTarget", { targetId, flatten: true });
const S = (m, p) => send(m, p, sessionId);
await S("Page.enable");
await S("Runtime.enable");
await S("Emulation.setDeviceMetricsOverride",
  { width: PAGE_W, height: SLICE_H, deviceScaleFactor: dpr, mobile: false });

const loaded = S("Page.navigate", { url: "file://" + htmlPath }).then(() => {});
await waitFor((e) => e.method === "Page.loadEventFired", 30000, "page load");
await loaded;

// settle: fonts + two frames + layout quiesce (anti-cache, deterministic)
await S("Runtime.evaluate", {
  awaitPromise: true, expression: `document.fonts.ready.then(() => new Promise(r => {
    requestAnimationFrame(() => requestAnimationFrame(r));
  }))`,
});
await new Promise((r) => setTimeout(r, 150));

const metrics = await S("Runtime.evaluate", {
  returnByValue: true,
  expression: `(() => {
    const de = document.documentElement, b = document.body;
    const h = Math.max(de.scrollHeight, b.scrollHeight,
                       de.getBoundingClientRect().height, b.getBoundingClientRect().height);
    const imgs = [...document.images];
    return { cssHeight: Math.ceil(h), cssWidth: Math.ceil(Math.max(
      de.scrollWidth, b.scrollWidth)),
      imgs: imgs.length, complete: imgs.every(i => i.complete && i.naturalWidth > 0),
      panels: imgs.map(i => { const r = i.getBoundingClientRect();
        const pr = i.closest('section') || i.parentElement;
        return { src: i.getAttribute('src'),
                 x: Math.round(r.left + window.scrollX), y: Math.round(r.top + window.scrollY),
                 w: Math.round(r.width), h: Math.round(r.height),
                 sectionY: Math.round(pr.getBoundingClientRect().top + window.scrollY) }; }) };
  })()`,
});
const m = metrics.result.value;
if (!m || !m.cssHeight || m.cssHeight < 1000) fail("page height implausible: " + JSON.stringify(m));
if (!m.complete) fail("one or more <img> panels did not decode");
if (m.cssWidth !== PAGE_W) fail(`page css width ${m.cssWidth} != ${PAGE_W}`);

// -- slices: full width, fixed height, y=0 in order --------------------------
const slices = Math.ceil(m.cssHeight / SLICE_H);
const manifest = { dpr, cssWidth: m.cssWidth, cssHeight: m.cssHeight,
                   sliceH: SLICE_H, slices, bitmaps: [], panels: m.panels };
for (let i = 0; i < slices; i++) {
  const y = i * SLICE_H;
  const h = Math.min(SLICE_H, m.cssHeight - y);
  const shot = await S("Page.captureScreenshot", {
    format: "png",
    captureBeyondViewport: true,
    // deviceScaleFactor already rasterizes at dpr; clip.scale multiplies on
    // top of it, so it must stay 1 (a scale here renders at dpr*scale).
    clip: { x: 0, y, width: PAGE_W, height: h, scale: 1 },
  });
  const name = `slice-${String(i).padStart(3, "0")}.png`;
  writeFileSync(join(outDir, name), Buffer.from(shot.data, "base64"));
  manifest.bitmaps.push({ name, cssY: y, cssH: h });
  console.log(`slice ${i + 1}/${slices} y=${y} h=${h}`);
}
writeFileSync(join(outDir, "manifest.json"), JSON.stringify(manifest, null, 1));

// expected bitmap height: css height * dpr, exactly
const expectH = m.cssHeight * dpr;
console.log(`captured: css ${m.cssWidth}x${m.cssHeight}, expect bitmap ${PAGE_W * dpr}x${expectH}`);

await send("Target.closeTarget", { targetId });
ws.close();
child.kill();
setTimeout(() => process.exit(0), 200);
