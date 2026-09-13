import assert from "node:assert/strict";
import { createServer } from "node:http";
import { request as httpRequest } from "node:http";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

const rendererModule = process.env.SCRIPTOR_PAGE_RENDERER_MODULE ?? "./page-renderer.mjs";
const { createPinnedProxy, launchBrowser, launchOptions, privateIp, protectContext, render } = await import(rendererModule);
const acquirerModule = process.env.SCRIPTOR_BINARY_ACQUIRER_MODULE ?? "./binary-acquirer.mjs";
const { acquire } = await import(acquirerModule);

assert.equal(privateIp("100.64.0.1"), true);
assert.equal(privateIp("198.18.0.1"), true);
assert.equal(privateIp("::7f00:1"), true);

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

const binaryRedirectServer = createServer((_request, response) => {
  response.writeHead(302, { location: "http://private.test/private" }).end();
});
await new Promise((resolve) => binaryRedirectServer.listen(0, "127.0.0.1", resolve));
const binaryRedirectAddress = binaryRedirectServer.address();
assert.ok(binaryRedirectAddress && typeof binaryRedirectAddress !== "string");
const binaryOutput = await mkdtemp(join(tmpdir(), "scriptor-acquirer-private-"));
try {
  const publicOnly = async (value) => {
    if (new URL(value).hostname !== "public.test") throw new Error("web_private_target_refused");
  };
  await assert.rejects(
    acquire(["--url", `http://public.test:${binaryRedirectAddress.port}/redirect`, "--output-dir", binaryOutput, "--max-download-bytes", "1024"], {
      assertPublic: publicOnly,
      createPinnedProxy: (limit) => createPinnedProxy(limit, async (host) => {
        if (host === "public.test") return "127.0.0.1";
        throw new Error("web_private_target_refused");
      }),
    }),
    /web_private_target_refused/,
    "the binary acquirer must refuse a private redirect through the pinned proxy",
  );
} finally {
  await rm(binaryOutput, { recursive: true, force: true });
  await new Promise((resolve) => binaryRedirectServer.close(resolve));
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

const fixtureServer = createServer((request, response) => {
  if (request.url === "/app.js") {
    response.writeHead(200, { "content-type": "application/javascript" }).end("setTimeout(() => document.body.insertAdjacentHTML('beforeend', '<p>SPA loaded</p>'), 0)");
    return;
  }
  if (request.url === "/frame") {
    response.writeHead(200, { "content-type": "text/html" }).end("<p>iframe content</p>");
    return;
  }
  response.writeHead(200, { "content-type": "text/html" }).end("<main><h1>Fixture page</h1><p>Static content</p><img src='/image.png'><audio src='/audio.mp3'></audio><video src='/video.mp4'></video><iframe src='/frame'></iframe><a href='/document.pdf'>Document</a><script src='/app.js'></script></main>");
});
await new Promise((resolve) => fixtureServer.listen(0, "127.0.0.1", resolve));
const fixtureAddress = fixtureServer.address();
assert.ok(fixtureAddress && typeof fixtureAddress !== "string");
const fixtureUrl = `http://public.test:${fixtureAddress.port}/`;
for (const browserName of ["firefox", "chromium"]) {
  const outputDir = await mkdtemp(join(tmpdir(), `scriptor-render-${browserName}-`));
  try {
    await render(["--output-dir", outputDir, "--url", fixtureUrl, "--browser", browserName, "--max-output-bytes", "1048576", "--max-download-bytes", "1048576"], {
      assertPublic: async () => undefined,
      createPinnedProxy: (limit) => createPinnedProxy(limit, async (host) => {
        if (host === "public.test") return "127.0.0.1";
        throw new Error("web_private_target_refused");
      }),
    });
    assert.match(await readFile(join(outputDir, "proofs/dom.html"), "utf8"), /Fixture page/);
    assert.match(await readFile(join(outputDir, "extractions/page.md"), "utf8"), /SPA loaded/);
    const screenshot = await readFile(join(outputDir, "proofs/screenshot.png"));
    assert.ok(screenshot.length > 0);
    const discoveries = JSON.parse(await readFile(join(outputDir, "discoveries.json"), "utf8"));
    assert.equal(discoveries.length, 5);
    assert.deepEqual(discoveries.map((discovery) => discovery.order), [0, 1, 2, 3, 4]);
  } finally {
    await rm(outputDir, { recursive: true, force: true });
  }
}
await new Promise((resolve) => fixtureServer.close(resolve));

await new Promise((resolve) => proxyServer.close(resolve));
