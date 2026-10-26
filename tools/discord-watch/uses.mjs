// What Neucord depends on, read from its own source so there is no list to
// keep: REST calls are `(method, path)` pairs in crates/discord, gateway
// events are the event names gateway.rs matches on.
//
// Calls are recognised by how they are written, so a new way of writing one
// would go unseen. Every HTTP method literal must therefore be accounted
// for; the rest are reported, and `compare` fails on them.

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
  const unaccounted = [];
  for (const file of await rustFiles(src)) {
    const code = await readFile(file, "utf8");
    const seen = new Set();
    const add = (method, path) => rest.set(`${method} ${normalizePath(path)}`, { method, path: normalizePath(path) });
    const PATH = String.raw`&?(?:format!\(\s*)?"(\/[^"]*)"`;
    // ("PUT", &format!("/users/@me/relationships/{user}")) and plain literals.
    for (const m of code.matchAll(new RegExp(String.raw`"(GET|POST|PUT|PATCH|DELETE)"\s*,\s*` + PATH, "g"))) {
      add(m[1], m[2]);
      seen.add(m.index);
    }
    // Helpers that fix the method and take the path: `fn auth(&self, path: &str, …)`
    // calling `self.send("POST", path, …)`, used as `self.auth("/auth/login", …)`.
    for (const m of code.matchAll(/fn (\w+)\(&self,\s*(\w+): &str[\s\S]*?\n    \}/g)) {
      const found = m[0].match(new RegExp(String.raw`"(GET|POST|PUT|PATCH|DELETE)"\s*,\s*${m[2]}\b`));
      const method = found?.[1];
      if (!method) continue;
      seen.add(m.index + found.index);
      for (const call of code.matchAll(new RegExp(String.raw`self\.${m[1]}\(\s*` + PATH, "g"))) add(method, call[1]);
    }
    for (const m of code.matchAll(/"(GET|POST|PUT|PATCH|DELETE)"/g)) {
      // Websocket handshakes (gateway, remote auth) are GETs, not REST calls.
      if (seen.has(m.index) || code.slice(m.index, m.index + 400).includes("websocket_connect")) continue;
      const line = code.slice(0, m.index).split("\n").length;
      const place = `${file.slice(repo.length + 1)}:${line}: ${code.split("\n")[line - 1].trim()}`;
      if (!unaccounted.includes(place)) unaccounted.push(place);
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
    unaccounted,
  };
}

/** Whether a client endpoint template (`/users/{0}/relationships`) can be
 * the path Neucord calls (`/users/@me/relationships`). Strictly, only the
 * template's parameters are wildcards; loosely, Neucord's `{}` also stands
 * for a literal (`/auth/mfa/{}` is totp, sms…), except `@me`. */
export function pathMatches(template, path, loose = false) {
  const a = normalizePath(template.replace(/\{\d+\}|\{\?\}/g, "{}")).split("/");
  const b = path.split("/");
  const fits = (x, y) => x === y || x === "{}" || (loose && y === "{}" && x !== "@me");
  return a.length === b.length && a.every((segment, i) => fits(segment, b[i]));
}

/** The names of the endpoints a Neucord path calls: exact matches, or the
 * loose ones if there are none. */
export function endpointsFor(endpoints, path) {
  const names = (loose) => Object.keys(endpoints).filter((name) => pathMatches(endpoints[name], path, loose));
  const strict = names(false);
  return strict.length > 0 ? strict : names(true);
}
