import { mkdir, stat, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { createServer as createHttpServer, request as httpRequest } from "node:http";
import { connect, isIP } from "node:net";
import { lookup } from "node:dns/promises";
import { fileURLToPath } from "node:url";
import { join } from "node:path";

const { chromium, firefox } = createRequire(import.meta.url)("playwright");
const privateV4 = (parts) => parts[0] === 10 || parts[0] === 127 || parts[0] === 0 || (parts[0] === 100 && parts[1] >= 64 && parts[1] <= 127) || (parts[0] === 169 && parts[1] === 254) || (parts[0] === 172 && parts[1] >= 16 && parts[1] <= 31) || (parts[0] === 192 && parts[1] === 168) || (parts[0] === 198 && (parts[1] === 18 || parts[1] === 19)) || parts[0] >= 224;
export const privateIp = (address) => {
  if (isIP(address) === 4) return privateV4(address.split(".").map(Number));
  const value = address.toLowerCase();
  const mappedV4 = value.match(/^(?:::|(?:0{1,4}:){5})ffff:(\d+\.\d+\.\d+\.\d+)$/)?.[1];
  if (mappedV4) return privateV4(mappedV4.split(".").map(Number));
  const mappedHex = value.match(/^(?:::|(?:0{1,4}:){5})ffff:([0-9a-f]{1,4}):([0-9a-f]{1,4})$/);
  if (mappedHex) { const high = Number.parseInt(mappedHex[1], 16); const low = Number.parseInt(mappedHex[2], 16); return privateV4([high >> 8, high & 255, low >> 8, low & 255]); }
  const compatibleHex = value.match(/^::([0-9a-f]{1,4}):([0-9a-f]{1,4})$/);
  if (compatibleHex) { const high = Number.parseInt(compatibleHex[1], 16); const low = Number.parseInt(compatibleHex[2], 16); return privateV4([high >> 8, high & 255, low >> 8, low & 255]); }
  return value === "::1" || value === "::" || value.startsWith("fc") || value.startsWith("fd") || value.startsWith("fe80:") || value.startsWith("ff");
};
const resolvedTarget = async (hostname, port) => {
  const host = hostname.replace(/^\[|\]$/g, "");
  if (!host || host.toLowerCase() === "localhost" || host.toLowerCase().endsWith(".localhost")) throw new Error("web_private_target_refused");
  const addresses = isIP(host) ? [{ address: host }] : await lookup(host, { all: true, verbatim: true });
  if (!addresses.length || addresses.some(({ address }) => privateIp(address))) throw new Error("web_private_target_refused");
  if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error("web_port_refused");
  return addresses[0].address;
};
export const assertPublic = async (value) => {
  const url = new URL(value);
  if (url.protocol !== "http:" && url.protocol !== "https:") throw new Error("web_url_scheme_refused");
  if (url.username || url.password) throw new Error("web_url_credentials_refused");
  await resolvedTarget(url.hostname, Number(url.port || (url.protocol === "https:" ? 443 : 80)));
};

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
export const protectContext = async (context, assertPublicUrl = assertPublic) => {
  await context.addInitScript(() => {
    for (const name of ["RTCPeerConnection", "webkitRTCPeerConnection", "RTCDataChannel"]) Object.defineProperty(globalThis, name, { configurable: false, value: undefined, writable: false });
  });
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

export const createPinnedProxy = async (limit, resolveTarget = resolvedTarget) => {
  let bytes = 0;
  let failure;
  const recordFailure = (error) => { failure ??= error instanceof Error ? error : new Error(String(error)); };
  const reserve = (size) => { bytes += size; if (bytes > limit) throw new Error("web_download_budget_exceeded"); };
  const server = createHttpServer(async (request, response) => {
    try {
      const target = new URL(request.url);
      if (target.protocol !== "http:" && target.protocol !== "ws:") throw new Error("web_proxy_scheme_refused");
      if (target.username || target.password || request.headers.upgrade) throw new Error("web_websocket_refused");
      const port = Number(target.port || 80);
      const ip = await resolveTarget(target.hostname, port);
      const upstream = httpRequest({ host: ip, port, method: request.method, path: `${target.pathname}${target.search}`, headers: { ...request.headers, host: target.host, connection: "close" } }, (upstreamResponse) => {
        response.writeHead(upstreamResponse.statusCode ?? 502, upstreamResponse.headers);
        upstreamResponse.on("data", (chunk) => { try { reserve(chunk.length); } catch (error) { recordFailure(error); upstream.destroy(); response.destroy(); } });
        upstreamResponse.pipe(response);
      });
      upstream.on("error", () => response.destroy()); request.pipe(upstream);
    } catch (error) { recordFailure(error); response.destroy(); }
  });
  server.on("connect", async (request, client, head) => {
    try {
      const split = request.url.lastIndexOf(":"); if (split < 1) throw new Error("web_proxy_target_refused");
      const host = request.url.slice(0, split); const port = Number(request.url.slice(split + 1));
      const upstream = connect({ host: await resolveTarget(host, port), port });
      upstream.once("connect", () => { client.write("HTTP/1.1 200 Connection Established\r\n\r\n"); if (head.length) upstream.write(head); for (const stream of [client, upstream]) stream.on("data", (chunk) => { try { reserve(chunk.length); } catch (error) { recordFailure(error); client.destroy(); upstream.destroy(); } }); client.pipe(upstream); upstream.pipe(client); });
      upstream.on("error", () => client.destroy());
    } catch (error) { recordFailure(error); client.destroy(); }
  });
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  const address = server.address(); if (!address || typeof address === "string") throw new Error("proxy address unavailable");
  return { server, address: `http://127.0.0.1:${address.port}`, failure: () => failure };
};
const option = (args, name, required = true) => { const index = args.indexOf(name); if (index < 0) { if (required) throw new Error(`missing ${name}`); return undefined; } if (!args[index + 1]) throw new Error(`missing ${name}`); return args[index + 1]; };
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
    browser = await launchBrowser(browserName, proxy.address);
    const context = await browser.newContext({ serviceWorkers: "block", viewport: { width: 1280, height: 720 }, permissions: [] });
    const routeRejection = await protectContext(context, assertPublicUrl); const page = await context.newPage();
    try { await page.goto(initialUrl, { waitUntil: "networkidle", timeout: 30_000 }); } catch (error) {
      const rejection = routeRejection() ?? proxy.failure();
      if (rejection) throw rejection;
      throw error;
    }
    const rejection = routeRejection() ?? proxy.failure(); if (rejection) throw rejection; await assertPublicUrl(page.url());
    const [dom, markdown, discoveries] = await page.evaluate(() => {
      const extensions = /\.(pdf|docx?|odt|rtf)$/i;
      const cssPath = (node) => { const parts = []; for (let current = node; current && current.nodeType === 1; current = current.parentElement) { const parent = current.parentElement; if (!parent) { parts.unshift(current.tagName.toLowerCase()); continue; } const siblings = [...parent.children].filter((item) => item.tagName === current.tagName); parts.unshift(`${current.tagName.toLowerCase()}:nth-of-type(${siblings.indexOf(current) + 1})`); } return parts.join(" > "); };
      const text = [...document.querySelectorAll("h1,h2,h3,h4,h5,h6,p,li,blockquote,pre")].map((node) => { const value = node.innerText.trim(); if (!value) return ""; if (/^H[1-6]$/.test(node.tagName)) return `${"#".repeat(Number(node.tagName[1]))} ${value}`; if (node.tagName === "LI") return `- ${value}`; if (node.tagName === "BLOCKQUOTE") return `> ${value}`; return value; }).filter(Boolean).join("\n\n");
      const findings = []; for (const node of document.querySelectorAll("img,audio,video,source,iframe,embed,object,a[href]")) { const attributes = node.tagName === "A" ? [node.getAttribute("href")] : [node.getAttribute("src"), node.getAttribute("data"), ...(node.getAttribute("srcset") ?? "").split(",").map((entry) => entry.trim().split(/\s+/)[0])]; for (const raw of attributes.filter(Boolean)) { const url = new URL(raw, document.baseURI).href; if (node.tagName !== "A" || extensions.test(new URL(url).pathname)) findings.push({ parent_locator: { kind: "url", value: document.baseURI }, locator: { kind: "css-selector", value: cssPath(node) }, url, order: findings.length, status: "inventoried", reason: node.tagName === "A" ? "linked_document" : "embedded_content" }); } } return [document.documentElement.outerHTML, text, findings];
    });
    await writeBudgeted(join(outputDir, "proofs/dom.html"), dom); await writeBudgeted(join(outputDir, "extractions/page.md"), markdown); await writeBudgeted(join(outputDir, "discoveries.json"), JSON.stringify(discoveries));
    await writeBudgeted(join(outputDir, "proofs/screenshot.png"), await page.screenshot({ type: "png" })); await writeBudgeted(join(outputDir, "provenance.json"), JSON.stringify({ initial_url: initialUrl, final_url: page.url(), browser: browserName }));
  } finally { if (browser) await browser.close(); await new Promise((resolve) => proxy.server.close(resolve)); }
};
if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) { try { await render(process.argv.slice(2)); } catch (error) { process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`); process.exitCode = 1; } }
