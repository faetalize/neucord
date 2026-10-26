// Distil a build into what a third-party client depends on: REST endpoint
// paths, the fields each REST call sends, the gateway events the client
// handles and the payload fields it reads, and the enums named in
// watch.json. Minified names are dropped, so snapshots of two builds differ
// only where Discord changed something.

import { readdir, readFile } from "node:fs/promises";
import { join } from "node:path";
import { parse, walk, isFunction, keyName, modulesOf } from "./js.mjs";

const WATCH = JSON.parse(await readFile(new URL("./watch.json", import.meta.url), "utf8"));
const METHODS = { get: "GET", post: "POST", put: "PUT", patch: "PATCH", del: "DELETE", delete: "DELETE" };
const CONSTANT = /^[A-Z][A-Z0-9_]*$/;

/** An endpoint value as a path template: parameters become `{0}`, `{1}`;
 * anything else computed becomes `{?}`. Null if it is not a path. */
function pathOf(value) {
  const params = isFunction(value)
    ? value.params.map((p) => (p.type === "AssignmentPattern" ? p.left.name : p.name))
    : [];
  const render = (node) => {
    switch (node?.type) {
      case "Literal":
        return typeof node.value === "string" ? node.value : String(node.value);
      case "TemplateLiteral":
        return node.quasis
          .map((q, i) => q.value.cooked + (i < node.expressions.length ? render(node.expressions[i]) : ""))
          .join("");
      case "BinaryExpression":
        return node.operator === "+" ? render(node.left) + render(node.right) : "{?}";
      case "Identifier":
        return params.includes(node.name) ? `{${params.indexOf(node.name)}}` : "{?}";
      default:
        return "{?}";
    }
  };
  let expression = value;
  if (isFunction(value)) {
    expression = value.body;
    if (expression.type === "BlockStatement") {
      expression = null;
      walk(value.body, (n) => {
        if (n !== value.body && isFunction(n)) return false;
        if (n.type === "ReturnStatement" && n.argument) expression = n.argument;
      });
    }
  }
  if (!expression) return null;
  const path = render(expression);
  return path.startsWith("/") ? path : null;
}

/** The keys of an object literal; spreads show as `...`. */
function keysOf(node) {
  if (node?.type !== "ObjectExpression") return node ? ["<value>"] : [];
  return node.properties.map((p) => (p.type === "SpreadElement" ? "..." : (keyName(p) ?? "[computed]"))).sort();
}

/** The payload fields a gateway handler reads from its first parameter. */
function fieldsRead(handler) {
  const param = handler.params[0];
  const fields = new Set();
  if (!param) return [];
  if (param.type === "ObjectPattern") {
    for (const p of param.properties) if (p.type === "Property") fields.add(keyName(p));
    return [...fields].sort();
  }
  if (param.type !== "Identifier") return [];
  walk(handler.body, (n) => {
    if (
      n.type === "MemberExpression" &&
      !n.computed &&
      n.object.type === "Identifier" &&
      n.object.name === param.name
    ) {
      fields.add(n.property.name);
    }
    if (
      n.type === "VariableDeclarator" &&
      n.id.type === "ObjectPattern" &&
      n.init?.type === "Identifier" &&
      n.init.name === param.name
    ) {
      for (const p of n.id.properties) if (p.type === "Property") fields.add(keyName(p));
    }
  });
  return [...fields].sort();
}

/** TypeScript enums as compiled: `(e={})[e.A=0]="A",e[e.B=1]="B"` for
 * numbers and `(e={}).A="a",e.B="b"` for strings. */
function enumsIn(sequence) {
  const groups = new Map();
  for (const expression of sequence.expressions) {
    if (expression.type !== "AssignmentExpression" || expression.left.type !== "MemberExpression") continue;
    const target = expression.left;
    const owner = (node) =>
      node.type === "Identifier"
        ? node.name
        : node.type === "AssignmentExpression" && node.left.type === "Identifier"
          ? node.left.name
          : null;
    let name, value, object;
    if (
      target.computed &&
      target.property.type === "AssignmentExpression" &&
      target.property.left.type === "MemberExpression"
    ) {
      // e[e.NAME = 1] = "NAME"
      name = target.property.left.property.name;
      value = target.property.right.value;
      object = owner(target.object);
      if (expression.right.value !== name) continue;
    } else if (!target.computed && expression.right.type === "Literal") {
      // e.NAME = "value"
      name = target.property.name;
      value = expression.right.value;
      object = owner(target.object);
    }
    if (!object || !CONSTANT.test(name ?? "") || value === undefined) continue;
    if (!groups.has(object)) groups.set(object, {});
    groups.get(object)[name] = value;
  }
  return [...groups.values()].filter((e) => Object.keys(e).length >= 2);
}

/** Sort object keys recursively, so the JSON is stable across runs. */
function sorted(value) {
  if (Array.isArray(value)) return value.map(sorted);
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.keys(value)
        .sort()
        .map((k) => [k, sorted(value[k])]),
    );
  }
  return value;
}

export async function snapshot(dir) {
  const meta = JSON.parse(await readFile(join(dir, "build.json"), "utf8"));
  const endpointTables = [];
  const calls = new Map();
  const gateway = new Map();
  const enums = new Map(Object.keys(WATCH.enums).map((name) => [name, new Set()]));
  let modules = 0;
  let unparsed = 0;

  for (const file of (await readdir(join(dir, "assets"))).sort()) {
    const source = await readFile(join(dir, "assets", file), "utf8");
    let ast;
    try {
      ast = parse(source);
    } catch {
      unparsed++;
      continue;
    }
    for (const { property } of modulesOf(ast)) {
      modules++;
      const body = property.value;
      const text = source.slice(body.start, body.end);
      const isGatewayModule = text.includes('"READY_SUPPLEMENTAL"') && text.includes('"INITIAL_GUILD"');
      walk(body, (node) => {
        if (node.type === "ObjectExpression" && node.properties.length >= 20) {
          const paths = {};
          for (const p of node.properties) {
            const name = keyName(p);
            const path = name && CONSTANT.test(name) ? pathOf(p.value) : null;
            if (path) paths[name] = path;
          }
          if (Object.keys(paths).length >= node.properties.length * 0.7) endpointTables.push(paths);
        }
        if (node.type === "CallExpression") {
          const callee = node.callee;
          const options = node.arguments[0];
          const method = callee.type === "MemberExpression" && !callee.computed && METHODS[callee.property.name];
          const url =
            options?.type === "ObjectExpression" && options.properties.find((p) => keyName(p) === "url")?.value;
          if (method && url) {
            const target = url.type === "CallExpression" ? url.callee : url;
            const endpoint =
              target.type === "MemberExpression" && !target.computed && CONSTANT.test(target.property.name)
                ? target.property.name
                : pathOf(url);
            if (endpoint) {
              const get = (key) => options.properties.find((p) => keyName(p) === key)?.value;
              const call = { body: keysOf(get("body")), query: keysOf(get("query")) };
              if (get("context")) call.context = true;
              if (get("headers")) call.headers = keysOf(get("headers"));
              const key = `${method} ${endpoint}`;
              if (!calls.has(key)) calls.set(key, new Set());
              calls.get(key).add(JSON.stringify(sorted(call)));
            }
          }
          // The gateway module registers handlers as f(["EVENT", ...], handler).
          const [events, handler] = node.arguments;
          if (
            isGatewayModule &&
            events?.type === "ArrayExpression" &&
            events.elements.length > 0 &&
            events.elements.every((e) => e?.type === "Literal" && CONSTANT.test(e.value)) &&
            isFunction(handler)
          ) {
            for (const event of events.elements) {
              if (!gateway.has(event.value)) gateway.set(event.value, new Set());
              for (const field of fieldsRead(handler)) gateway.get(event.value).add(field);
            }
          }
        }
        if (node.type === "SequenceExpression") {
          for (const found of enumsIn(node)) {
            for (const [name, anchors] of Object.entries(WATCH.enums)) {
              if (anchors.every((a) => Object.hasOwn(found, a))) enums.get(name).add(JSON.stringify(sorted(found)));
            }
          }
        }
      });
    }
  }

  // The API endpoint table is the one REST calls name the most; others
  // (app routes, asset paths) share its shape.
  const named = new Set([...calls.keys()].map((k) => k.split(" ")[1]));
  const score = (table) => Object.keys(table).filter((k) => named.has(k)).length;
  const endpoints = endpointTables.sort((a, b) => score(b) - score(a))[0] ?? {};

  return sorted({
    channel: meta.channel,
    build: meta.build,
    stats: { scripts: meta.scripts, modules, unparsed },
    endpoints,
    calls: Object.fromEntries([...calls].map(([k, v]) => [k, [...v].sort().map((s) => JSON.parse(s))])),
    gateway: Object.fromEntries([...gateway].map(([k, v]) => [k, [...v].sort()])),
    enums: Object.fromEntries([...enums].map(([k, v]) => [k, [...v].sort().map((s) => JSON.parse(s))])),
  });
}
