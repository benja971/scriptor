import assert from "node:assert/strict";
import { createServer } from "node:http";
import { request as httpRequest } from "node:http";

const rendererModule = process.env.SCRIPTOR_PAGE_RENDERER_MODULE ?? "./page-renderer.mjs";
const { createPinnedProxy, launchBrowser, launchOptions, privateIp, protectContext } = await import(rendererModule);

assert.equal(privateIp("100.64.0.1"), true);
assert.equal(privateIp("198.18.0.1"), true);

const proxyServer = createServer((_request, response) => response.writeHead(502).end());
await new Promise((resolve) => proxyServer.listen(0, "127.0.0.1", resolve));
const address = proxyServer.address();
assert.ok(address && typeof address !== "string");
const proxy = `http://127.0.0.1:${address.port}`;

const privateProxy = await createPinnedProxy(1024);
await new Promise((resolve) => {
  const request = httpRequest(privateProxy.address, { path: "http://127.0.0.1/private" });
  request.on("error", resolve);
  request.end();
});
assert.equal(privateProxy.failure()?.message, "web_private_target_refused", "the pinned proxy must refuse a private resource before a connection");
await new Promise((resolve) => privateProxy.server.close(resolve));

const redirectServer = createServer((_request, response) => {
  response.writeHead(302, { location: "http://127.0.0.1/private" }).end();
});
await new Promise((resolve) => redirectServer.listen(0, "127.0.0.1", resolve));
const redirectAddress = redirectServer.address();
assert.ok(redirectAddress && typeof redirectAddress !== "string");
const redirectProxy = await createPinnedProxy(1024, async (host) => {
  if (host === "public.test") return "127.0.0.1";
  throw new Error("web_private_target_refused");
});
const redirectBrowser = await launchBrowser("firefox", redirectProxy.address);
try {
  const page = await redirectBrowser.newPage();
  await page.goto(`http://public.test:${redirectAddress.port}/redirect`).catch(() => undefined);
  assert.equal(redirectProxy.failure()?.message, "web_private_target_refused", "a private redirect must be refused by the proxy before a connection");
} finally {
  await redirectBrowser.close();
  await new Promise((resolve) => redirectProxy.server.close(resolve));
  await new Promise((resolve) => redirectServer.close(resolve));
}

for (const browserName of ["firefox", "chromium"]) {
  const options = launchOptions(browserName, proxy);
  if (browserName === "chromium") {
    assert.ok(options.args.includes("--force-webrtc-ip-handling-policy=disable_non_proxied_udp"));
  } else {
    assert.equal(options.firefoxUserPrefs["media.peerconnection.enabled"], false);
  }

  const browser = await launchBrowser(browserName, proxy);
  try {
    const context = await browser.newContext({ permissions: [] });
    const rejection = await protectContext(context);
    const page = await context.newPage();
    await page.setContent("<script>window.rtc = typeof RTCPeerConnection</script>");
    assert.equal(await page.evaluate(() => window.rtc), "undefined", `${browserName} must not expose WebRTC to page JavaScript`);
    assert.equal(rejection(), undefined);

    await context.close();
  } finally {
    await browser.close();
  }
}

await new Promise((resolve) => proxyServer.close(resolve));
