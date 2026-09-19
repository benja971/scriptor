import { mkdir, stat, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { createServer } from "node:net";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { join } from "node:path";

import { assertPublic, createPinnedProxy, privateIp } from "./web-safety.mjs";

const { chromium, firefox } = createRequire(import.meta.url)("playwright");
export { assertPublic, createPinnedProxy, privateIp };

// Proxying alone is insufficient: WebRTC can send UDP outside it. Firefox
// disables PeerConnection. Chromium forbids non-proxied UDP at process launch.
export const launchOptions = (name, proxyAddress) => {
  const proxy = { server: proxyAddress };
  if (name === "firefox") return { headless: true, proxy, firefoxUserPrefs: { "media.peerconnection.enabled": false, "media.peerconnection.ice.default_address_only": true, "media.peerconnection.ice.no_host": true, "media.peerconnection.ice.proxy_only": true } };
  if (name === "chromium") return { headless: true, proxy, args: ["--force-webrtc-ip-handling-policy=disable_non_proxied_udp", "--webrtc-ip-handling-policy=disable_non_proxied_udp", "--enforce-webrtc-ip-permission-check"] };
  throw new Error("web_renderer_unknown");
};
export const launchBrowser = async (name, proxyAddress) => {
  const launcher = name === "firefox" ? firefox : name === "chromium" ? chromium : undefined;
  if (!launcher) throw new Error("web_renderer_unknown");
  try { return await launcher.launch(launchOptions(name, proxyAddress)); } catch (error) {
    const detail = error instanceof Error ? error.message : String(error);
    throw new Error(`web_renderer_${name}_unavailable: ${detail}`);
  }
};
const disableWebRtc = async (context) => context.addInitScript(() => {
  for (const name of ["RTCPeerConnection", "webkitRTCPeerConnection", "RTCDataChannel"]) Object.defineProperty(globalThis, name, { configurable: false, value: undefined, writable: false });
});
export const protectContext = async (context, assertPublicUrl = assertPublic) => {
  await disableWebRtc(context);
  let rejection;
  await context.route("**/*", async (route) => {
    try { await assertPublicUrl(route.request().url()); await route.continue(); } catch (error) {
      rejection = error instanceof Error ? error : new Error(String(error));
      await route.abort("blockedbyclient");
    }
  });
  await context.routeWebSocket("**/*", async (socket) => { rejection = new Error("web_websocket_refused"); await socket.close({ code: 1008, reason: "WebSocket capture is disabled" }); });
  return () => rejection;
};
const settleLazyContent = async (page) => {
  let previousHeight = -1;
  let stablePasses = 0;
  for (let pass = 0; pass < 10 && stablePasses < 2; pass += 1) {
    await page.evaluate(() => scrollTo(0, document.documentElement.scrollHeight));
    await page.waitForTimeout(100);
    const height = await page.evaluate(() => document.documentElement.scrollHeight);
    stablePasses = height === previousHeight ? stablePasses + 1 : 0;
    previousHeight = height;
  }
  await page.evaluate(() => scrollTo(0, 0));
};

const option = (args, name, required = true) => { const index = args.indexOf(name); if (index < 0) { if (required) throw new Error(`missing ${name}`); return undefined; } if (!args[index + 1]) throw new Error(`missing ${name}`); return args[index + 1]; };
const availablePort = async () => new Promise((resolve, reject) => {
  const server = createServer();
  server.once("error", reject);
  server.listen(0, "127.0.0.1", () => {
    const address = server.address();
    if (!address || typeof address === "string") { server.close(); reject(new Error("lightpanda address unavailable")); return; }
    server.close((error) => error ? reject(error) : resolve(address.port));
  });
});
const pause = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));
const stopLightpanda = async (child) => {
  if (child.exitCode !== null) return;
  await new Promise((resolve) => { child.once("close", resolve); child.kill(); });
};
const startLightpanda = async (proxyAddress) => {
  const port = await availablePort();
  let detail = "";
  const child = spawn("lightpanda", ["serve", "--host", "127.0.0.1", "--port", String(port), "--http-proxy", proxyAddress, "--disable-metrics"], { env: { ...process.env, LIGHTPANDA_DISABLE_TELEMETRY: "true" }, stdio: ["ignore", "ignore", "pipe"] });
  child.stderr.on("data", (chunk) => { detail = `${detail}${chunk}`.slice(-4096); });
  let startupError;
  child.once("error", (error) => { startupError = error; });
  try {
    for (let attempt = 0; attempt < 250; attempt += 1) {
      if (startupError) throw startupError;
      if (child.exitCode !== null) throw new Error(detail || "lightpanda exited during startup");
      try { return { browser: await chromium.connectOverCDP(`http://127.0.0.1:${port}`), child }; } catch { await pause(20); }
    }
    throw new Error(detail || "lightpanda startup timed out");
  } catch (error) {
    await stopLightpanda(child);
    const message = error instanceof Error ? error.message : String(error);
    throw new Error(`web_renderer_lightpanda_unavailable: ${message}`);
  }
};
const renderLightpanda = async ({ initialUrl, outputDir, proxy, assertPublicUrl, writeBudgeted }) => {
  const { browser, child } = await startLightpanda(proxy.address);
  let context;
  try {
    context = await browser.newContext({ serviceWorkers: "block", permissions: [] });
    await disableWebRtc(context);
    const page = await context.newPage();
    const navigation = await page.goto(initialUrl, { waitUntil: "load", timeout: 30_000 });
    const failure = proxy.failure(); if (failure) throw failure;
    if (navigation && !navigation.ok()) throw new Error(`web_navigation_failed: ${navigation.status()}`);
    await assertPublicUrl(page.url()); await settleLazyContent(page);
    const lazyFailure = proxy.failure(); if (lazyFailure) throw lazyFailure;
    const dom = await page.evaluate(() => document.documentElement.outerHTML);
    const session = await context.newCDPSession(page);
    const { markdown } = await session.send("LP.getMarkdown", {});
    const redirects = []; for (let request = navigation?.request(); request; request = request.redirectedFrom()) redirects.unshift(request.url());
    await writeBudgeted(join(outputDir, "proofs/dom.html"), dom); await writeBudgeted(join(outputDir, "extractions/page.md"), markdown); await writeBudgeted(join(outputDir, "provenance.json"), JSON.stringify({ initial_url: initialUrl, final_url: page.url(), redirect_chain: redirects, browser: "lightpanda" }));
  } finally {
    try {
      if (context) await context.close();
    } finally {
      try {
        await browser.close();
      } finally {
        await stopLightpanda(child);
      }
    }
  }
};
export const render = async (args, dependencies = {}) => {
  const assertPublicUrl = dependencies.assertPublic ?? assertPublic;
  const createProxy = dependencies.createPinnedProxy ?? createPinnedProxy;
  const outputDir = option(args, "--output-dir"); const initialUrl = option(args, "--url"); const browserName = option(args, "--browser", false) ?? "firefox";
  const maxOutputBytes = Number(option(args, "--max-output-bytes")); const maxDownloadBytes = Number(option(args, "--max-download-bytes"));
  if (!Number.isSafeInteger(maxOutputBytes) || maxOutputBytes < 1 || !Number.isSafeInteger(maxDownloadBytes) || maxDownloadBytes < 1) throw new Error("invalid renderer budget");
  let artifactBytes = 0;
  const writeBudgeted = async (path, value) => { const bytes = Buffer.byteLength(value); if (artifactBytes + bytes > maxOutputBytes) throw new Error("web_disk_budget_exceeded"); await writeFile(path, value); artifactBytes += bytes; if ((await stat(path)).size !== bytes) throw new Error("web_disk_budget_exceeded"); };
  await assertPublicUrl(initialUrl); await mkdir(join(outputDir, "proofs"), { recursive: true }); await mkdir(join(outputDir, "extractions"), { recursive: true });
  const proxy = await createProxy(maxDownloadBytes); let browser;
  try {
    if (browserName === "lightpanda") {
      await renderLightpanda({ initialUrl, outputDir, proxy, assertPublicUrl, writeBudgeted });
      return;
    }
    browser = await launchBrowser(browserName, proxy.address);
    const context = await browser.newContext({ serviceWorkers: "block", viewport: { width: 1280, height: 720 }, permissions: [] });
    const routeRejection = await protectContext(context, assertPublicUrl); const page = await context.newPage();
    let navigation; try { navigation = await page.goto(initialUrl, { waitUntil: "networkidle", timeout: 30_000 }); } catch (error) {
      const rejection = routeRejection() ?? proxy.failure();
      if (rejection) throw rejection;
      throw error;
    }
    const rejection = routeRejection() ?? proxy.failure(); if (rejection) throw rejection;
    if (navigation && !navigation.ok()) throw new Error(`web_navigation_failed: ${navigation.status()}`);
    await assertPublicUrl(page.url()); await settleLazyContent(page);
    const lazyRejection = routeRejection() ?? proxy.failure(); if (lazyRejection) throw lazyRejection;
    const [dom, markdown, discoveries] = await page.evaluate(() => {
      const extensions = /\.(pdf|docx?|odt|rtf)$/i;
      const cssPath = (node) => { const parts = []; for (let current = node; current && current.nodeType === 1; current = current.parentElement) { const parent = current.parentElement; if (!parent) { parts.unshift(current.tagName.toLowerCase()); continue; } const siblings = [...parent.children].filter((item) => item.tagName === current.tagName); parts.unshift(`${current.tagName.toLowerCase()}:nth-of-type(${siblings.indexOf(current) + 1})`); } return parts.join(" > "); };
      const text = [...document.querySelectorAll("h1,h2,h3,h4,h5,h6,p,li,blockquote,pre")].map((node) => { const value = node.innerText.trim(); if (!value) return ""; if (/^H[1-6]$/.test(node.tagName)) return `${"#".repeat(Number(node.tagName[1]))} ${value}`; if (node.tagName === "LI") return `- ${value}`; if (node.tagName === "BLOCKQUOTE") return `> ${value}`; return value; }).filter(Boolean).join("\n\n");
      const findings = []; for (const node of document.querySelectorAll("img,audio,video,source,iframe,embed,object,a[href]")) { const attributes = node.tagName === "A" ? [node.getAttribute("href")] : [node.getAttribute("src"), node.getAttribute("data"), ...(node.getAttribute("srcset") ?? "").split(",").map((entry) => entry.trim().split(/\s+/)[0])]; for (const raw of attributes.filter(Boolean)) { const url = new URL(raw, document.baseURI).href; if (node.tagName !== "A" || extensions.test(new URL(url).pathname)) findings.push({ parent_locator: { kind: "url", value: document.baseURI }, locator: { kind: "css-selector", value: cssPath(node) }, url, order: findings.length, status: "inventoried", reason: node.tagName === "A" ? "linked_document" : "embedded_content", ...(node.tagName === "IFRAME" ? { kind: "web" } : {}) }); } } return [document.documentElement.outerHTML, text, findings];
    });
    await writeBudgeted(join(outputDir, "proofs/dom.html"), dom); await writeBudgeted(join(outputDir, "extractions/page.md"), markdown); await writeBudgeted(join(outputDir, "discoveries.json"), JSON.stringify(discoveries));
    const redirects = []; for (let request = navigation?.request(); request; request = request.redirectedFrom()) redirects.unshift(request.url()); await writeBudgeted(join(outputDir, "proofs/screenshot.png"), await page.screenshot({ type: "png" })); await writeBudgeted(join(outputDir, "provenance.json"), JSON.stringify({ initial_url: initialUrl, final_url: page.url(), redirect_chain: redirects, browser: browserName }));
  } finally { if (browser) await browser.close(); await new Promise((resolve) => proxy.server.close(resolve)); }
};
if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) { try { await render(process.argv.slice(2)); } catch (error) { process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`); process.exitCode = 1; } }
