import { mkdir, stat, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { createServer as createHttpServer, request as httpRequest } from "node:http";
import { connect } from "node:net";
import { isIP } from "node:net";
import { lookup } from "node:dns/promises";
import { join } from "node:path";

const require = createRequire(import.meta.url);
const { chromium, firefox } = require("playwright");
const args = process.argv.slice(2);
const option = (name) => { const index = args.indexOf(name); if (index < 0 || !args[index + 1]) throw new Error(`missing ${name}`); return args[index + 1]; };
const outputDir = option("--output-dir");
const initialUrl = option("--url");
const maxOutputBytes = Number(option("--max-output-bytes"));
const maxDownloadBytes = Number(option("--max-download-bytes"));
if (!Number.isSafeInteger(maxOutputBytes) || maxOutputBytes < 1 || !Number.isSafeInteger(maxDownloadBytes) || maxDownloadBytes < 1) throw new Error("invalid renderer budget");

const privateV4 = (parts) => parts[0] === 10 || parts[0] === 127 || parts[0] === 0 || (parts[0] === 169 && parts[1] === 254) || (parts[0] === 172 && parts[1] >= 16 && parts[1] <= 31) || (parts[0] === 192 && parts[1] === 168) || parts[0] >= 224;
const privateIp = (address) => { if (isIP(address) === 4) return privateV4(address.split(".").map(Number)); const value = address.toLowerCase(); return value === "::1" || value === "::" || value.startsWith("fc") || value.startsWith("fd") || value.startsWith("fe80:") || value.startsWith("ff") || value.startsWith("::ffff:127.") || value.startsWith("::ffff:10.") || value.startsWith("::ffff:192.168."); };
const resolvedTarget = async (hostname, port) => {
  if (!hostname || hostname.toLowerCase() === "localhost" || hostname.toLowerCase().endsWith(".localhost")) throw new Error("web_private_target_refused");
  const addresses = isIP(hostname) ? [{ address: hostname }] : await lookup(hostname, { all: true, verbatim: true });
  if (!addresses.length || addresses.some(({ address }) => privateIp(address))) throw new Error("web_private_target_refused");
  if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error("web_port_refused");
  return addresses[0].address;
};
const assertPublic = async (value) => { const url = new URL(value); if (url.protocol !== "http:" && url.protocol !== "https:") throw new Error("web_url_scheme_refused"); if (url.username || url.password) throw new Error("web_url_credentials_refused"); await resolvedTarget(url.hostname, Number(url.port || (url.protocol === "https:" ? 443 : 80))); };

// The explicit proxy is the network trust boundary: it resolves each host once,
// rejects non-public addresses and dials that numeric address, pinning DNS.
const createPinnedProxy = async () => {
  let relayedBytes = 0;
  const reserve = (size) => { relayedBytes += size; if (relayedBytes > maxDownloadBytes) throw new Error("web_download_budget_exceeded"); };
  const server = createHttpServer(async (clientRequest, clientResponse) => {
    try {
      const target = new URL(clientRequest.url);
      if (target.protocol !== "http:" && target.protocol !== "ws:") throw new Error("web_proxy_scheme_refused");
      if (target.username || target.password) throw new Error("web_url_credentials_refused");
      const port = Number(target.port || 80); if (port !== 80) throw new Error("web_port_refused");
      const ip = await resolvedTarget(target.hostname, port);
      const upstream = httpRequest({ host: ip, port, method: clientRequest.method, path: `${target.pathname}${target.search}`, headers: { ...clientRequest.headers, host: target.host, connection: "close" } }, (response) => {
        clientResponse.writeHead(response.statusCode ?? 502, response.headers);
        response.on("data", (chunk) => { try { reserve(chunk.length); } catch { upstream.destroy(); clientResponse.destroy(); } });
        response.pipe(clientResponse);
      });
      upstream.on("error", () => clientResponse.destroy()); clientRequest.pipe(upstream);
    } catch { clientResponse.destroy(); }
  });
  server.on("connect", async (request, clientSocket, head) => {
    try {
      const separator = request.url.lastIndexOf(":"); if (separator < 1) throw new Error("web_proxy_target_refused");
      const hostname = request.url.slice(0, separator); const port = Number(request.url.slice(separator + 1)); if (port !== 443) throw new Error("web_port_refused");
      const ip = await resolvedTarget(hostname, port); const upstream = connect({ host: ip, port });
      upstream.once("connect", () => {
        clientSocket.write("HTTP/1.1 200 Connection Established\r\n\r\n"); if (head.length) upstream.write(head);
        for (const stream of [clientSocket, upstream]) stream.on("data", (chunk) => { try { reserve(chunk.length); } catch { clientSocket.destroy(); upstream.destroy(); } });
        clientSocket.pipe(upstream); upstream.pipe(clientSocket);
      });
      upstream.on("error", () => clientSocket.destroy());
    } catch { clientSocket.destroy(); }
  });
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  const address = server.address(); if (!address || typeof address === "string") throw new Error("proxy address unavailable");
  return { server, address: `http://127.0.0.1:${address.port}` };
};
let artifactBytes = 0;
const reserveArtifact = async (path) => { artifactBytes += (await stat(path)).size; if (artifactBytes > maxOutputBytes) throw new Error("web_disk_budget_exceeded"); };
const writeBudgeted = async (path, value) => { await writeFile(path, value); await reserveArtifact(path); };
try {
  await assertPublic(initialUrl); await mkdir(join(outputDir, "proofs"), { recursive: true }); await mkdir(join(outputDir, "extractions"), { recursive: true });
  const proxy = await createPinnedProxy(); let browser; let browserName = "firefox";
  try { browser = await firefox.launch({ headless: true, proxy: { server: proxy.address } }); } catch { browserName = "chromium"; browser = await chromium.launch({ headless: true, proxy: { server: proxy.address } }); }
  try {
    const context = await browser.newContext({ serviceWorkers: "block" });
    await context.route("**/*", async (route) => { try { await assertPublic(route.request().url()); await route.continue(); } catch { await route.abort("blockedbyclient"); } });
    const page = await context.newPage(); await page.goto(initialUrl, { waitUntil: "networkidle", timeout: 30_000 }); await assertPublic(page.url());
    const [dom, markdown, discoveries] = await page.evaluate(() => {
      const extensions = /\.(pdf|docx?|odt|rtf)$/i;
      const cssPath = (node) => { const parts = []; for (let current = node; current && current.nodeType === 1; current = current.parentElement) { const siblings = [...(current.parentElement?.children ?? [])].filter((item) => item.tagName === current.tagName); parts.unshift(`${current.tagName.toLowerCase()}:nth-of-type(${siblings.indexOf(current) + 1})`); } return parts.join(" > "); };
      const text = [...document.querySelectorAll("h1,h2,h3,h4,h5,h6,p,li,blockquote,pre")].map((node) => { const value = node.innerText.trim(); if (!value) return ""; if (/^H[1-6]$/.test(node.tagName)) return `${"#".repeat(Number(node.tagName[1]))} ${value}`; if (node.tagName === "LI") return `- ${value}`; if (node.tagName === "BLOCKQUOTE") return `> ${value}`; return value; }).filter(Boolean).join("\n\n");
      const findings = []; for (const node of document.querySelectorAll("img,audio,video,source,iframe,embed,object,a[href]")) { const attributes = node.tagName === "A" ? [node.getAttribute("href")] : [node.getAttribute("src"), node.getAttribute("data"), ...(node.getAttribute("srcset") ?? "").split(",").map((entry) => entry.trim().split(/\s+/)[0])]; for (const raw of attributes.filter(Boolean)) { const url = new URL(raw, document.baseURI).href; if (node.tagName !== "A" || extensions.test(new URL(url).pathname)) findings.push({ type: node.tagName.toLowerCase(), locator: cssPath(node), url, status: "inventoried", reason: node.tagName === "A" ? "linked_document" : "embedded_content" }); } }
      return [document.documentElement.outerHTML, text, findings];
    });
    await writeBudgeted(join(outputDir, "proofs/dom.html"), dom); await writeBudgeted(join(outputDir, "extractions/page.md"), markdown); await writeBudgeted(join(outputDir, "discoveries.json"), JSON.stringify(discoveries)); await page.screenshot({ path: join(outputDir, "proofs/screenshot.png"), fullPage: true }); await reserveArtifact(join(outputDir, "proofs/screenshot.png")); await writeBudgeted(join(outputDir, "provenance.json"), JSON.stringify({ initial_url: initialUrl, final_url: page.url(), browser: browserName }));
  } finally { await browser.close(); await new Promise((resolve) => proxy.server.close(resolve)); }
} catch (error) { process.stderr.write(`${error.message}\n`); process.exitCode = 1; }
