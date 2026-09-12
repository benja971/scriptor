import { mkdir, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { isIP } from "node:net";
import { lookup } from "node:dns/promises";
import { join } from "node:path";

const require = createRequire(import.meta.url);
const { chromium, firefox } = require("playwright");

const args = process.argv.slice(2);
const option = (name) => {
  const index = args.indexOf(name);
  if (index < 0 || !args[index + 1]) throw new Error(`missing ${name}`);
  return args[index + 1];
};
const outputDir = option("--output-dir");
const initialUrl = option("--url");

const privateV4 = (parts) => parts[0] === 10 || parts[0] === 127 || parts[0] === 0 ||
  (parts[0] === 169 && parts[1] === 254) || (parts[0] === 172 && parts[1] >= 16 && parts[1] <= 31) ||
  (parts[0] === 192 && parts[1] === 168) || parts[0] >= 224;
const privateIp = (address) => {
  if (isIP(address) === 4) return privateV4(address.split(".").map(Number));
  const lowered = address.toLowerCase();
  return lowered === "::1" || lowered === "::" || lowered.startsWith("fc") || lowered.startsWith("fd") || lowered.startsWith("fe80:") || lowered.startsWith("ff");
};
const assertPublic = async (value) => {
  const url = new URL(value);
  if (url.protocol !== "http:" && url.protocol !== "https:") throw new Error("web_url_scheme_refused");
  if (url.username || url.password) throw new Error("web_url_credentials_refused");
  if (url.hostname === "localhost" || url.hostname.endsWith(".localhost")) throw new Error("web_private_target_refused");
  if (isIP(url.hostname)) {
    if (privateIp(url.hostname)) throw new Error("web_private_target_refused");
    return;
  }
  const addresses = await lookup(url.hostname, { all: true, verbatim: true });
  if (!addresses.length || addresses.some(({ address }) => privateIp(address))) throw new Error("web_private_target_refused");
};
try {
  await assertPublic(initialUrl);
  await mkdir(join(outputDir, "proofs"), { recursive: true });
  await mkdir(join(outputDir, "extractions"), { recursive: true });
  let browser;
  let browserName = "firefox";
  try { browser = await firefox.launch({ headless: true }); }
  catch { browserName = "chromium"; browser = await chromium.launch({ headless: true }); }
  const context = await browser.newContext({ serviceWorkers: "block" });
  await context.route("**/*", async (route) => {
    try { await assertPublic(route.request().url()); await route.continue(); }
    catch { await route.abort("blockedbyclient"); }
  });
  const page = await context.newPage();
  await page.goto(initialUrl, { waitUntil: "networkidle", timeout: 30_000 });
  await assertPublic(page.url());
  const [dom, pageMarkdown, discoveries] = await page.evaluate(() => {
    const selectors = "img, audio, video, source, iframe, embed, object, a[href]";
    const isDocument = (url) => /\.(pdf|docx?|odt|rtf)$/i.test(new URL(url).pathname);
    const markdown = [...document.querySelectorAll("h1,h2,h3,h4,h5,h6,p,li,blockquote,pre")]
      .map((node) => {
        const text = node.innerText.trim();
        if (!text) return "";
        if (/^H[1-6]$/.test(node.tagName)) return `${"#".repeat(Number(node.tagName[1]))} ${text}`;
        if (node.tagName === "LI") return `- ${text}`;
        if (node.tagName === "BLOCKQUOTE") return `> ${text}`;
        return text;
      }).filter(Boolean).join("\n\n");
    return [document.documentElement.outerHTML, markdown, [...document.querySelectorAll(selectors)]
      .map((node) => ({ node, url: new URL(node.getAttribute("src") || node.getAttribute("href") || node.getAttribute("data"), document.baseURI).href }))
      .filter(({ node, url }) => node.tagName !== "A" || isDocument(url))
      .map(({ node, url }, order) => ({ order, parent_locator: document.baseURI, locator: node.outerHTML, url, status: "inventoried", reason: "embedded_content" }))];
  });
  await writeFile(join(outputDir, "proofs/dom.html"), dom);
  await writeFile(join(outputDir, "extractions/page.md"), pageMarkdown);
  await writeFile(join(outputDir, "discoveries.json"), JSON.stringify(discoveries));
  await page.screenshot({ path: join(outputDir, "proofs/screenshot.png"), fullPage: true });
  await writeFile(join(outputDir, "provenance.json"), JSON.stringify({ initial_url: initialUrl, final_url: page.url(), browser: browserName }));
  await browser.close();
} catch (error) {
  process.stderr.write(`${error.message}\n`);
  process.exitCode = 1;
}
