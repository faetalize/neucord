// What Neucord depends on, read from its own source so there is no list to
// keep: REST calls are `(method, path)` pairs in crates/discord, gateway
// events are the event names gateway.rs matches on.

import { readdir, readFile } from "node:fs/promises";
import { join } from "node:path";

async function rustFiles(dir) {
  const out = [];
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) out.push(...(await rustFiles(path)));
    else if (entry.name.endsWith(".rs")) out.push(path);
  }
  return out;
}

/** `/channels/{channel}/messages?limit={limit}` → `/channels/{}/messages`. */
export function normalizePath(path) {
  return path.split("?")[0].replace(/\{[^}]*\}/g, "{}");
}

export async function neucordUses(repo) {
  const src = join(repo, "crates", "discord", "src");
  const rest = new Map();
  const events = new Set();
  for (const file of await rustFiles(src)) {
    const code = await readFile(file, "utf8");
    // ("PUT", &format!("/users/@me/relationships/{user}")) and plain literals.
    for (const m of code.matchAll(/"(GET|POST|PUT|PATCH|DELETE)"\s*,\s*&?(?:format!\(\s*)?"(\/[^"]*)"/g)) {
      const key = `${m[1]} ${normalizePath(m[2])}`;
      rest.set(key, { method: m[1], path: normalizePath(m[2]) });
    }
    if (file.endsWith("gateway.rs")) {
      // Match arms: `"MESSAGE_CREATE" =>` and `"READY" | "RESUMED"`.
      for (const m of code.matchAll(/"([A-Z][A-Z0-9_]*)"\s*(?==>|\|)/g)) events.add(m[1]);
      for (const m of code.matchAll(/\|\s*"([A-Z][A-Z0-9_]*)"/g)) events.add(m[1]);
    }
  }
  return {
    rest: [...rest.values()].sort((a, b) => `${a.path} ${a.method}`.localeCompare(`${b.path} ${b.method}`)),
    events: [...events].sort(),
  };
}

/** Whether a client endpoint template (`/users/{0}/relationships`) can be
 * the path Neucord calls (`/users/@me/relationships`). */
export function pathMatches(template, path) {
  const a = normalizePath(template.replace(/\{\d+\}|\{\?\}/g, "{}")).split("/");
  const b = path.split("/");
  // Only the template's parameters are wildcards: Neucord's `{}` (an id)
  // must not match a literal like `@me`.
  return a.length === b.length && a.every((segment, i) => segment === b[i] || segment === "{}");
}
