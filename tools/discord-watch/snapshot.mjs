// Distil a build into what a third-party client depends on: REST endpoint
// paths, the fields each REST call sends, the headers added to every
// request, the gateway events the client handles and the payload fields it
// reads, and the enums, methods and constants named in watch.json. Minified
// names are dropped, so snapshots of two builds differ only where Discord
// changed something.

import { readdir, readFile } from "node:fs/promises";
import { join } from "node:path";
import { parse, walk, isFunction, keyName, modulesOf } from "./js.mjs";

const WATCH = JSON.parse(await readFile(new URL("./watch.json", import.meta.url), "utf8"));
const METHODS = { get: "GET", post: "POST", put: "PUT", patch: "PATCH", del: "DELETE", delete: "DELETE" };
const CONSTANT = /^[A-Z][A-Z0-9_]*$/;
const HEADER = /^[A-Za-z][A-Za-z0-9-]*$/;

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
  // Single members too: only anchors pick from these, and Discord has
  // one-member enums (`UPDATE_PASSWORD`).
  return [...groups.values()];
}

/** A function's source with its minified names replaced: its own variables
 * become a0, a1… in order of appearance, and anything from another module
 * (`I.rbe.DM`, `(0, _.pp)(…)`) becomes `<import>`, keeping what is read from
 * it (`<import>.DM`). Whitespace goes too, so the text only changes with the
 * code. */
function normalized(source, fn) {
  const bound = new Set();
  walk(fn, (n) => {
    const patterns = isFunction(n) || n.type === "FunctionDeclaration" ? [...n.params, n.id] : [];
    if (n.type === "VariableDeclarator") patterns.push(n.id);
    if (n.type === "CatchClause") patterns.push(n.param);
    for (const p of patterns.filter(Boolean)) walk(p, (m) => void (m.type === "Identifier" && bound.add(m.name)));
  });
  const names = new Set();
  const edits = [];
  walk(fn, (n) => {
    if (n.type === "MemberExpression" && !n.computed) {
      names.add(n.property);
      const root = n.object;
      if (root.type === "Identifier" && !bound.has(root.name) && !(root.name in globalThis)) {
        edits.push({ start: root.start, end: n.end, text: "<import>" });
        return false;
      }
    }
    if ((n.type === "Property" && !n.computed && !n.shorthand) || n.type === "MethodDefinition") names.add(n.key);
    if (n.type !== "Identifier" || names.has(n)) return;
    if (bound.has(n.name)) edits.push({ start: n.start, end: n.end, local: n.name });
    else if (!(n.name in globalThis)) edits.push({ start: n.start, end: n.end, text: "<import>" });
  });
  const locals = new Map();
  let text = "";
  let at = fn.start;
  for (const edit of edits.sort((a, b) => a.start - b.start)) {
    if (edit.start < at) continue;
    if (edit.local && !locals.has(edit.local)) locals.set(edit.local, `a${locals.size}`);
    text += source.slice(at, edit.start) + (edit.text ?? locals.get(edit.local));
    at = edit.end;
  }
  text += source.slice(at, fn.end);
  return text.replace(/\s*([^\w\s$])\s*/g, "$1").replace(/\s+/g, " ");
}

/** A constant's value: a literal, or an array or `new Set([…])` of them
 * (a set sorted, as its order means nothing). Undefined for anything else. */
function constantOf(node) {
  if (node?.type === "Literal") return node.value;
  const literals = (array) =>
    array?.type === "ArrayExpression" && array.elements.every((e) => e?.type === "Literal")
      ? array.elements.map((e) => e.value)
      : undefined;
  if (node?.type === "ArrayExpression") return literals(node);
  if (node?.type === "NewExpression" && node.callee.type === "Identifier" && node.callee.name === "Set") {
    return literals(node.arguments[0])?.sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));
  }
  return undefined;
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
  const headers = new Set();
  const enums = new Map(Object.keys(WATCH.enums).map((name) => [name, new Set()]));
  const methods = new Map(WATCH.methods.map((name) => [name, new Set()]));
  const constants = new Map(WATCH.constants.map((name) => [name, new Set()]));
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
      // Constants are named where they are used or exported, and often
      // defined apart: `MAX: () => c` in the export map, `c = 30` below it.
      const declared = new Map();
      const named = [];
      walk(body, (node) => {
        if (node.type === "VariableDeclarator" && node.id.type === "Identifier" && node.init) {
          declared.set(node.id.name, node.init);
        }
        if (node.type === "Property" && constants.has(keyName(node))) named.push([keyName(node), node.value]);
        if (node.type === "ObjectExpression" && node.properties.length >= 20) {
          const paths = {};
          for (const p of node.properties) {
            const name = keyName(p);
            const path = name && CONSTANT.test(name) ? pathOf(p.value) : null;
            if (path) paths[name] = path;
          }
          if (Object.keys(paths).length >= node.properties.length * 0.7) endpointTables.push(paths);
        }
        // The HTTP layer's hook, which sets headers on every API request:
        // prepareRequest(e) { …; e.set("X-Super-Properties", n); … }
        if (node.type === "Property" && keyName(node) === "prepareRequest" && isFunction(node.value)) {
          walk(node.value.body, (n) => {
            const name = n.type === "CallExpression" ? n.arguments[0] : null;
            if (
              n.callee?.type === "MemberExpression" &&
              !n.callee.computed &&
              n.callee.property.name === "set" &&
              name?.type === "Literal" &&
              HEADER.test(name.value)
            ) {
              headers.add(name.value);
            }
          });
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
        // Class methods and object methods alike: `isSystemDM() { … }`.
        if ((node.type === "MethodDefinition" || node.type === "Property") && isFunction(node.value)) {
          const name = !node.computed && node.key.type === "Identifier" ? node.key.name : null;
          if (methods.has(name)) methods.get(name).add(normalized(source, node.value));
        }
        if (node.type === "SequenceExpression") {
          for (const found of enumsIn(node)) {
            for (const [name, anchors] of Object.entries(WATCH.enums)) {
              if (anchors.every((a) => Object.hasOwn(found, a))) enums.get(name).add(JSON.stringify(sorted(found)));
            }
          }
        }
      });
      for (const [name, value] of named) {
        const target =
          value.type === "ArrowFunctionExpression" && value.body.type === "Identifier"
            ? declared.get(value.body.name)
            : value;
        const found = constantOf(target);
        if (found !== undefined) constants.get(name).add(JSON.stringify(found));
      }
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
    headers: [...headers].sort(),
    enums: Object.fromEntries([...enums].map(([k, v]) => [k, [...v].sort().map((s) => JSON.parse(s))])),
    methods: Object.fromEntries([...methods].map(([k, v]) => [k, [...v].sort()])),
    constants: Object.fromEntries([...constants].map(([k, v]) => [k, [...v].sort().map((s) => JSON.parse(s))])),
  });
}
