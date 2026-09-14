import { createServer as createHttpServer, request as httpRequest } from "node:http";
import { connect, isIP } from "node:net";
import { lookup } from "node:dns/promises";

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
