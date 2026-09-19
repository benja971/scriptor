import { createHash } from "node:crypto";
import { execFile } from "node:child_process";
import { createReadStream } from "node:fs";
import { mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { promisify } from "node:util";

import { assertPublic, createPinnedProxy } from "./web-safety.mjs";

const execFileAsync = promisify(execFile);

const option = (args, name) => {
  const index = args.indexOf(name);
  if (index < 0 || !args[index + 1]) throw new Error(`missing ${name}`);
  return args[index + 1];
};

const options = (args, name) => args.flatMap((value, index) => value === name && args[index + 1] ? [args[index + 1]] : []);

export const acquire = async (args, dependencies = {}) => {
  const assertPublicUrl = dependencies.assertPublic ?? assertPublic;
  const createProxy = dependencies.createPinnedProxy ?? createPinnedProxy;
  const url = option(args, "--url");
  const outputDir = option(args, "--output-dir");
  const maxDownloadBytes = Number(option(args, "--max-download-bytes"));
  if (!Number.isSafeInteger(maxDownloadBytes) || maxDownloadBytes < 1) throw new Error("invalid acquirer budget");
  await assertPublicUrl(url);
  await mkdir(outputDir, { recursive: true });
  const payload = join(outputDir, "payload");
  const headers = join(outputDir, "headers");
  const proxy = await createProxy(maxDownloadBytes);
  try {
    let stdout;
    try {
      ({ stdout } = await execFileAsync("curl", [
        "-q",
        "--proxy", proxy.address,
        "--location", "--max-redirs", "5",
        "--proto", "=http,https", "--proto-redir", "=http,https",
        "--fail", "--silent", "--show-error",
        ...options(args, "--header").flatMap((header) => ["--header", header]),
        "--output", payload,
        "--dump-header", headers,
        "--write-out", "%{content_type}\\n%{url_effective}",
        url,
      ], { encoding: "utf8" }));
    } catch (error) {
      throw proxy.failure() ?? error;
    }
    const failure = proxy.failure();
    if (failure) throw failure;
    const [mime, finalUrl] = stdout.split("\n");
    await assertPublicUrl(finalUrl);
    const redirectChain = [url];
    let redirectBase = url;
    for (const line of (await readFile(headers, "utf8")).split("\n")) {
      const location = line.match(/^location:\s*(.+)\r?$/i)?.[1];
      if (location) {
        redirectBase = new URL(location, redirectBase).href;
        await assertPublicUrl(redirectBase);
        redirectChain.push(redirectBase);
      }
    }
    if (redirectChain.at(-1) !== finalUrl) redirectChain.push(finalUrl);
    const info = await stat(payload);
    if (info.size > maxDownloadBytes) throw new Error("web_download_budget_exceeded");
    const hash = createHash("sha256");
    for await (const chunk of createReadStream(payload)) hash.update(chunk);
    await writeFile(join(outputDir, "metadata.json"), JSON.stringify({
      requested_url: url,
      final_url: finalUrl,
      mime,
      redirect_chain: redirectChain,
      sha256: hash.digest("hex"),
      size_bytes: info.size,
    }));
    await rm(headers, { force: true });
  } finally {
    await new Promise((resolve) => proxy.server.close(resolve));
  }
};

if (process.argv[1]?.endsWith("binary-acquirer.mjs")) {
  try { await acquire(process.argv.slice(2)); }
  catch (error) { process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`); process.exitCode = 1; }
}
