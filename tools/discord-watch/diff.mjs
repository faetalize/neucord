// Compare two snapshots and write the report: first what touches Neucord
// (calls it makes, headers on every request, events it handles, enum values
// it may rely on, rules and values it copies), then a summary of everything
// else.

import { endpointsFor } from "./uses.mjs";

const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
const code = (s) => `\`${s}\``;
const MAX_LINES = 80;

function describeCalls(calls) {
  if (!calls) return "no call in the client";
  return calls
    .map((c) => {
      const parts = [`body {${c.body.join(", ")}}`];
      if (c.query.length > 0) parts.push(`query {${c.query.join(", ")}}`);
      if (c.headers) parts.push(`headers {${c.headers.join(", ")}}`);
      if (c.context) parts.push("context");
      return parts.join(", ");
    })
    .join(" | ");
}

function listChange(before = [], after = []) {
  const added = after.filter((x) => !before.includes(x));
  const removed = before.filter((x) => !after.includes(x));
  return [
    added.length > 0 && `added ${added.map(code).join(", ")}`,
    removed.length > 0 && `removed ${removed.map(code).join(", ")}`,
  ]
    .filter(Boolean)
    .join("; ");
}

function capped(lines) {
  return lines.length <= MAX_LINES ? lines : [...lines.slice(0, MAX_LINES), `- … and ${lines.length - MAX_LINES} more`];
}

/** Calls Neucord makes that match no endpoint of the client: renamed in
 * Neucord's favour, or never parsed. Worth a look, but not every week. */
export function unmatched(cur, uses) {
  return uses.rest.filter(({ path }) => endpointsFor(cur.endpoints, path).length === 0);
}

export function compare(old, cur, uses) {
  const affects = [];

  for (const { method, path } of uses.rest) {
    const names = new Set([...endpointsFor(old.endpoints, path), ...endpointsFor(cur.endpoints, path)]);
    for (const name of [...names].sort()) {
      const [before, after] = [old.endpoints[name], cur.endpoints[name]];
      const label = `${code(`${method} ${path}`)} (${code(name)})`;
      if (before !== after) {
        affects.push(`- ${label}: path ${before ? code(before) : "(new)"} → ${after ? code(after) : "**removed**"}`);
      }
      const key = `${method} ${name}`;
      if (!same(old.calls[key], cur.calls[key])) {
        affects.push(
          `- ${label}: the client's request changed\n  - was: ${describeCalls(old.calls[key])}\n  - now: ${describeCalls(cur.calls[key])}`,
        );
      }
    }
  }

  // Every request goes through the client's HTTP layer, so a header it
  // starts or stops adding may soon be expected of Neucord's requests too.
  // Snapshots from before headers were recorded have none to compare.
  const headers = old.headers && listChange(old.headers, cur.headers);
  if (headers) affects.push(`- Headers the client adds to every request: ${headers}`);

  for (const event of uses.events) {
    const [before, after] = [old.gateway[event], cur.gateway[event]];
    if (same(before, after)) continue;
    if (before && !after) affects.push(`- ${code(event)}: the client no longer handles it`);
    else if (!before)
      affects.push(`- ${code(event)}: the client now handles it, reading ${after.map(code).join(", ") || "nothing"}`);
    else affects.push(`- ${code(event)}: fields the client reads: ${listChange(before, after)}`);
  }

  // The methods in watch.json are rules Neucord copies, such as where the
  // client shows no message box; if one changes, Neucord may now disagree.
  // Snapshots from before methods were recorded have none to compare.
  const rule = (bodies) => (bodies?.length ? bodies.map(code).join(" | ") : "not found");
  for (const name of Object.keys({ ...old.methods, ...cur.methods }).sort()) {
    const [before, after] = [old.methods?.[name], cur.methods?.[name]];
    if (!old.methods || same(before, after)) continue;
    affects.push(`- ${code(name)}: the client's rule changed\n  - was: ${rule(before)}\n  - now: ${rule(after)}`);
  }

  // The stores in watch.json hold rules too, spread over a module too long
  // to quote: the full diff shows what changed.
  for (const name of Object.keys({ ...old.stores, ...cur.stores }).sort()) {
    const [before, after] = [old.stores?.[name], cur.stores?.[name]];
    if (!old.stores || same(before, after)) continue;
    affects.push(`- ${code(name)}: ${after?.length ? "the client's rule changed" : "not found"}`);
  }

  // The constants in watch.json are values Neucord copies, such as how
  // many emoji a message can be and still show them large.
  const value = (values) => (values?.length ? values.map((v) => code(JSON.stringify(v))).join(" | ") : "not found");
  for (const name of Object.keys({ ...old.constants, ...cur.constants }).sort()) {
    const [before, after] = [old.constants?.[name], cur.constants?.[name]];
    if (!old.constants || same(before, after)) continue;
    affects.push(`- ${code(name)}: the client's value changed\n  - was: ${value(before)}\n  - now: ${value(after)}`);
  }

  const additions = [];
  for (const name of Object.keys({ ...old.enums, ...cur.enums }).sort()) {
    const merge = (variants = []) => Object.assign({}, ...variants);
    const [before, after] = [merge(old.enums[name]), merge(cur.enums[name])];
    for (const member of Object.keys(before)) {
      if (!(member in after)) affects.push(`- ${code(name)}: ${code(member)} (${before[member]}) removed`);
      else if (before[member] !== after[member])
        affects.push(`- ${code(name)}: ${code(member)} ${before[member]} → ${after[member]}`);
    }
    const added = Object.keys(after).filter((m) => !(m in before));
    if (added.length > 0) additions.push(`- ${code(name)}: ${added.map((m) => `${code(m)} = ${after[m]}`).join(", ")}`);
  }

  const other = [];
  const section = (title, lines) =>
    lines.length > 0 && other.push(`**${title}** (${lines.length})`, ...capped(lines), "");
  const endpointNames = Object.keys({ ...old.endpoints, ...cur.endpoints }).sort();
  section(
    "Endpoints added",
    endpointNames.filter((n) => !(n in old.endpoints)).map((n) => `- ${code(n)}: ${code(cur.endpoints[n])}`),
  );
  section(
    "Endpoints removed",
    endpointNames.filter((n) => !(n in cur.endpoints)).map((n) => `- ${code(n)}: ${code(old.endpoints[n])}`),
  );
  section(
    "Endpoint paths changed",
    endpointNames
      .filter((n) => n in old.endpoints && n in cur.endpoints && old.endpoints[n] !== cur.endpoints[n])
      .map((n) => `- ${code(n)}: ${code(old.endpoints[n])} → ${code(cur.endpoints[n])}`),
  );
  const callKeys = Object.keys({ ...old.calls, ...cur.calls }).sort();
  section(
    "Requests changed",
    callKeys
      .filter((k) => !same(old.calls[k], cur.calls[k]))
      .map((k) => `- ${code(k)}: ${describeCalls(old.calls[k])} → ${describeCalls(cur.calls[k])}`),
  );
  const events = Object.keys({ ...old.gateway, ...cur.gateway }).sort();
  section(
    "Gateway events added",
    events.filter((e) => !(e in old.gateway)).map((e) => `- ${code(e)}`),
  );
  section(
    "Gateway events removed",
    events.filter((e) => !(e in cur.gateway)).map((e) => `- ${code(e)}`),
  );
  section("Enum members added", additions);

  return { affects, other };
}

/** The issue body for a comparison. */
export function report(old, cur, { affects, other }) {
  const lines = [
    `Discord's ${cur.channel} client went from build ${old.build} to ${cur.build}.`,
    "",
    "## What Neucord uses",
    "",
    ...(affects.length > 0 ? affects : ["Nothing Neucord calls or handles changed."]),
    "",
    "## Everything else",
    "",
    ...(other.length > 0 ? ["<details><summary>Show</summary>", "", ...other, "</details>"] : ["No other changes."]),
    "",
    "Found by comparing what the official client sends and reads; the client may lag or lead the API. `tools/discord-watch` has the details.",
  ];
  let body = lines.join("\n");
  // GitHub caps issue bodies at 65536 characters.
  if (body.length > 60000) body = body.slice(0, 60000) + "\n\n… (cut; see the snapshot diff)";
  return body;
}
