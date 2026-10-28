// Download one build of Discord's web client: the scripts the app page
// loads, plus every lazily loaded chunk, found through webpack's chunk-URL
// function. The desktop apps load this same client from discord.com, so
// their downloads hold none of it.

import { access, mkdir, readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { parse, walk, isFunction, evaluate } from "./js.mjs";

const ORIGINS = {
  stable: "https://discord.com",
  ptb: "https://ptb.discord.com",
  canary: "https://canary.discord.com",
};
// A plain tool name: no account, cookies or contact details go to Discord.
const HEADERS = { "User-Agent": "neucord-discord-watch" };

async function get(url, attempts = 3) {
  for (let attempt = 1; ; attempt++) {
    try {
      const response = await fetch(url, { headers: HEADERS });
      if (response.ok) return Buffer.from(await response.arrayBuffer());
      if (response.status === 404 || attempt === attempts) throw new Error(`HTTP ${response.status}`);
    } catch (error) {
      if (attempt === attempts) throw new Error(`${url}: ${error.message}`);
    }
    await new Promise((resolve) => setTimeout(resolve, 1000 * attempt));
  }
}

/** Chunk file names from webpack's `__webpack_require__.u`, a function of
 * the chunk id made of id comparisons and an id → hash table. */
export function chunkFiles(source) {
  const files = new Set();
  walk(parse(source), (node) => {
    if (
      node.type !== "AssignmentExpression" ||
      node.left.type !== "MemberExpression" ||
      node.left.property.name !== "u" ||
      !isFunction(node.right) ||
      node.right.params.length !== 1 ||
      node.right.body.type === "BlockStatement"
    ) {
      return;
    }
    const param = node.right.params[0].name;
    const ids = new Set();
    walk(node.right.body, (n) => {
      if (n.type === "BinaryExpression" && n.operator === "===" && n.left.type === "Literal")
        ids.add(String(n.left.value));
      if (n.type === "ObjectExpression")
        for (const p of n.properties) if (p.key) ids.add(String(p.key.value ?? p.key.name));
    });
    for (const id of ids) {
      try {
        const file = evaluate(node.right.body, { [param]: id });
        if (typeof file === "string" && /^[\w.-]+\.js$/.test(file)) files.add(file);
      } catch {
        // Not this function's kind of expression; skip the id.
      }
    }
    return false;
  });
  return files;
}

async function appPage(channel) {
  const origin = ORIGINS[channel];
  if (!origin) throw new Error(`unknown channel ${channel}`);
  const page = (await get(`${origin}/app`)).toString("utf8");
  const build = page.match(/BUILD_NUMBER"\s*:\s*"(\d+)"/)?.[1] ?? "unknown";
  // The API and gateway version the client speaks, from the page's GLOBAL_ENV.
  const apiVersion = Number(page.match(/"API_VERSION"\s*:\s*(\d+)/)?.[1]) || null;
  return { origin, page, build, apiVersion };
}

/** The build number `channel` serves now, without downloading it. */
export async function currentBuild(channel = "stable") {
  return (await appPage(channel)).build;
}

/** Fetch `channel`'s current build into `dir/assets`. Returns its metadata. */
export async function fetchBuild(dir, { channel = "stable", log = () => {} } = {}) {
  const { origin, page, build, apiVersion } = await appPage(channel);
  const entries = new Set([...page.matchAll(/(?:src|href)="\/assets\/([\w.-]+\.js)"/g)].map((m) => m[1]));
  if (entries.size === 0) throw new Error(`no scripts on ${origin}/app; did the page change?`);

  const assets = join(dir, "assets");
  await mkdir(assets, { recursive: true });
  const download = async (file) => {
    const path = join(assets, file);
    try {
      await access(path);
      return;
    } catch {}
    await writeFile(path, await get(`${origin}/assets/${file}`));
  };
  const pool = async (files) => {
    const queue = [...files];
    let failed = 0;
    await Promise.all(
      Array.from({ length: 16 }, async () => {
        while (queue.length > 0) {
          const file = queue.pop();
          try {
            await download(file);
          } catch (error) {
            failed++;
            log(`skipped ${error.message}`);
          }
        }
      }),
    );
    return failed;
  };

  log(`build ${build}: ${entries.size} entry scripts`);
  let failed = await pool(entries);
  const chunks = new Set();
  for (const file of entries) {
    const source = await readFile(join(assets, file), "utf8").catch(() => "");
    if (!source.includes(".u=")) continue;
    try {
      for (const chunk of chunkFiles(source)) if (!entries.has(chunk)) chunks.add(chunk);
    } catch {
      // Unparsable entry; it carries no chunk map we can read.
    }
  }
  if (chunks.size === 0) throw new Error("found no chunk map; the webpack runtime changed shape");
  log(`build ${build}: ${chunks.size} chunks`);
  failed += await pool(chunks);

  const meta = { channel, build, apiVersion, scripts: entries.size + chunks.size, failed };
  await writeFile(join(dir, "build.json"), JSON.stringify(meta, null, 2) + "\n");
  return meta;
}
