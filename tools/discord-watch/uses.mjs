// What Neucord depends on, read from its own source so there is no list to
// keep: REST calls are the table in crates/discord/src/endpoints.rs, which
// every request goes through; gateway events are the event names gateway.rs
// matches on; the API version is identity.rs's API_BASE.

import { readFile } from "node:fs/promises";
import { join } from "node:path";

/** `/channels/{channel}/messages?limit={0}` → `/channels/{}/messages`. */
export function normalizePath(path) {
  return path.split("?")[0].replace(/\{[^}]*\}/g, "{}");
}

export async function neucordUses(repo) {
  const src = join(repo, "crates", "discord", "src");
  const table = await readFile(join(src, "endpoints.rs"), "utf8");
  const rest = [];
  const unread = [];
  // `pub(crate) const SEND_MESSAGE: Endpoint = Endpoint::post("/channels/{channel}/messages");`
  const ROW = /^pub\(crate\) const (\w+): Endpoint = Endpoint::(get|post|put|patch|delete)\("(\/[^"]*)"\);$/;
  for (const line of table.split("\n").filter((line) => line.startsWith("pub(crate) const"))) {
    const [, name, method, path] = line.match(ROW) ?? [];
    if (name) rest.push({ name, method: method.toUpperCase(), path: normalizePath(path) });
    else unread.push(line);
  }
  if (rest.length === 0) unread.push("no endpoints in endpoints.rs");

  // The API version: `pub const API_BASE: &str = "https://discord.com/api/v9";`
  const identity = await readFile(join(src, "identity.rs"), "utf8");
  const apiVersion = Number(identity.match(/API_BASE: &str = "https:\/\/discord\.com\/api\/v(\d+)"/)?.[1]) || null;
  if (!apiVersion) unread.push("no API version in identity.rs");

  const events = new Set();
  const gateway = await readFile(join(src, "gateway.rs"), "utf8");
  // Match arms: `"MESSAGE_CREATE" =>` and `"READY" | "RESUMED"`.
  for (const m of gateway.matchAll(/"([A-Z][A-Z0-9_]*)"\s*(?==>|\|)/g)) events.add(m[1]);
  for (const m of gateway.matchAll(/\|\s*"([A-Z][A-Z0-9_]*)"/g)) events.add(m[1]);

  return {
    rest: rest.sort((a, b) => `${a.path} ${a.method}`.localeCompare(`${b.path} ${b.method}`)),
    events: [...events].sort(),
    apiVersion,
    unread,
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
