// Distil a build into what a third-party client depends on: REST endpoint
// paths, the fields each REST call sends, the headers added to every
// request, the gateway events the client handles and the payload fields it
// reads, the enums, methods, stores and constants named in watch.json, and
// every experiment the client defines.
// Minified names are dropped, so snapshots of two builds differ only where
// Discord changed something.

import { readdir, readFile } from "node:fs/promises";
import { join } from "node:path";
import { parse, walk, isFunction, keyName, modulesOf } from "./js.mjs";

const WATCH = JSON.parse(await readFile(new URL("./watch.json", import.meta.url), "utf8"));
// The modules MANUAL.md has people check by hand, by id.
const MANUAL = new Set((await readFile(new URL("./MANUAL.md", import.meta.url), "utf8")).match(/\b\d{4,7}\b/g) ?? []);
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

/** Flag tables written as objects: `Object.freeze({ PINNED: 2 })`, or
 * `{ VIEW_CHANNEL: x.vI(10) }` for permissions, whose bits outgrow a number,
 * kept as the bit's position. Null for any other object. */
function objectEnum(node) {
  const found = {};
  for (const p of node.properties) {
    const name = p.type === "Property" && !p.computed ? keyName(p) : null;
    if (!name || !CONSTANT.test(name)) return null;
    const value = p.value.type === "CallExpression" && p.value.arguments.length === 1 ? p.value.arguments[0] : p.value;
    if (value.type !== "Literal" || typeof value.value !== "number") return null;
    found[name] = value.value;
  }
  return Object.keys(found).length > 0 ? found : null;
}

/** The identifiers a pattern declares: `a`, `{ b: [c] }`, `...d`, `e = 1`. */
function declaredBy(pattern) {
  switch (pattern?.type) {
    case "Identifier":
      return [pattern];
    case "ObjectPattern":
      return pattern.properties.flatMap((p) => declaredBy(p.type === "RestElement" ? p.argument : p.value));
    case "ArrayPattern":
      return pattern.elements.flatMap(declaredBy);
    case "RestElement":
      return declaredBy(pattern.argument);
    case "AssignmentPattern":
      return declaredBy(pattern.left);
    default:
      return [];
  }
}

const BLOCKS = new Set(["BlockStatement", "ForStatement", "ForInStatement", "ForOfStatement", "SwitchStatement"]);

/** Where each identifier in `root` is declared, if within `root`: a map
 * from identifier to the identifier declaring it. Minifiers reuse a name in
 * scopes that don't see each other, so the name alone can't say whether `i`
 * is an import or a local. `var` and parameters belong to their function;
 * `let`, `const`, classes and functions to their block. */
function declarations(root) {
  const scopes = new Map();
  const declare = (scope, ids) => {
    if (!scopes.has(scope)) scopes.set(scope, new Map());
    for (const id of ids) if (!scopes.get(scope).has(id.name)) scopes.get(scope).set(id.name, id);
  };
  const children = (node) =>
    Object.values(node)
      .flat()
      .filter((c) => c && typeof c.type === "string");
  // Declarations first, as they hoist: a scope is a function or a block node.
  const hoist = (node, fn, block) => {
    if (isFunction(node) || node.type === "FunctionDeclaration") {
      if (node.type === "FunctionDeclaration" && node.id) declare(block, [node.id]);
      if (node.type === "FunctionExpression" && node.id) declare(node, [node.id]);
      declare(node, node.params.flatMap(declaredBy));
      for (const child of [...node.params, ...(node.body.type === "BlockStatement" ? node.body.body : [node.body])])
        hoist(child, node, node);
      return;
    }
    if (node.type === "VariableDeclaration") {
      declare(
        node.kind === "var" ? fn : block,
        node.declarations.flatMap((d) => declaredBy(d.id)),
      );
    }
    if (node.type === "ClassDeclaration" && node.id) declare(block, [node.id]);
    if (node.type === "CatchClause") declare(node, declaredBy(node.param));
    const inner = BLOCKS.has(node.type) || node.type === "CatchClause" ? node : block;
    for (const child of children(node)) hoist(child, fn, inner);
  };
  hoist(root, root, root);
  const found = new Map();
  const resolve = (node, chain) => {
    // A function declaration's name belongs to the scope around it.
    const name = node.type === "FunctionDeclaration" ? node.id : null;
    if (name) resolve(name, chain);
    if (scopes.has(node)) chain = [node, ...chain];
    if (node.type === "Identifier") {
      const scope = chain.find((s) => scopes.get(s).has(node.name));
      if (scope) found.set(node, scopes.get(scope).get(node.name));
    }
    for (const child of children(node)) if (child !== name) resolve(child, chain);
  };
  resolve(root, []);
  return found;
}

/** A function's source with its minified names replaced: its own variables
 * become a0, a1… by name, in order of appearance, and anything from another
 * module (`I.rbe.DM`, `(0, _.pp)(…)`) becomes `<import>`, keeping what is
 * read from it (`<import>.DM`). Whitespace goes too, so the text only changes with the
 * code. A whole webpack module takes its imports (`r = n(17928)`) for
 * another module's, and its export map for `<exports>`, as both are named
 * by the minifier. */
function normalized(source, fn, { module = false } = {}) {
  const require = module ? fn.params[2]?.name : undefined;
  const declared = declarations(fn);
  const imports = new Set();
  walk(fn, (n) => {
    const init = n.type === "VariableDeclarator" ? n.init : null;
    if (require && init?.type === "CallExpression" && init.callee.type === "Identifier" && init.callee.name === require)
      imports.add(n.id);
  });
  // Declared within, and not as an import.
  const local = (id) => declared.has(id) && !imports.has(declared.get(id));
  const names = new Set();
  const edits = [];
  walk(fn, (n) => {
    const callee = n.type === "CallExpression" ? n.callee : null;
    if (
      require &&
      callee?.type === "MemberExpression" &&
      callee.object.name === require &&
      callee.property.name === "d"
    ) {
      edits.push({ start: n.start, end: n.end, text: "<exports>" });
      return false;
    }
    if (n.type === "MemberExpression" && !n.computed) {
      names.add(n.property);
      const root = n.object;
      if (root.type === "Identifier" && !local(root) && !(root.name in globalThis)) {
        edits.push({ start: root.start, end: n.end, text: "<import>" });
        return false;
      }
    }
    const keyed = ["MethodDefinition", "PropertyDefinition"].includes(n.type) && !n.computed;
    if ((n.type === "Property" && !n.computed && !n.shorthand) || keyed) names.add(n.key);
    if (n.type !== "Identifier" || names.has(n)) return;
    if (local(n)) edits.push({ start: n.start, end: n.end, local: n.name });
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

/** An experiment config as data: literals, objects and arrays as they are,
 * `!0` and `void 0` as what they mean, `new X(…)` and calls by their
 * arguments, and anything named by the minifier as `<import>` or
 * `<function>`, which a build can rename. */
function valueOf(node) {
  switch (node?.type) {
    case "Literal":
      return node.value;
    case "TemplateLiteral":
      return node.expressions.length === 0 ? node.quasis[0].value.cooked : "<template>";
    case "UnaryExpression":
      if (node.operator === "!" && node.argument.type === "Literal") return !node.argument.value;
      if (node.operator === "void") return null;
      if (node.operator === "-" && node.argument.type === "Literal") return -node.argument.value;
      return "<expression>";
    case "ObjectExpression":
      return Object.fromEntries(
        node.properties.map((p) =>
          p.type === "SpreadElement" ? ["...", "<spread>"] : [keyName(p) ?? "[computed]", valueOf(p.value)],
        ),
      );
    case "ArrayExpression":
      return node.elements.map(valueOf);
    case "NewExpression":
    case "CallExpression":
      return { "<call>": node.arguments.map(valueOf) };
    case "ArrowFunctionExpression":
    case "FunctionExpression":
      return "<function>";
    default:
      return node ? "<import>" : null;
  }
}

/** An experiment definition: Apex `{ name, kind, defaultConfig, variations }`
 * or the older `{ kind, id, label, defaultConfig, treatments }`. */
function experimentOf(node) {
  if (node?.type !== "ObjectExpression") return null;
  const get = (key) => node.properties.find((p) => keyName(p) === key)?.value;
  const kind = get("kind");
  if (kind?.type !== "Literal" || !["user", "guild", "installation"].includes(kind.value)) return null;
  const literal = (key) => (get(key)?.type === "Literal" ? get(key).value : null);
  if (typeof literal("name") === "string" && get("variations")) {
    const variants = get("variations").type === "ObjectExpression" ? get("variations").properties : [];
    return [
      literal("name"),
      {
        system: "apex",
        kind: kind.value,
        default: valueOf(get("defaultConfig")),
        variants: Object.fromEntries(variants.map((p) => [keyName(p) ?? "[computed]", valueOf(p.value)])),
      },
    ];
  }
  if (typeof literal("id") === "string" && get("treatments")?.type === "ArrayExpression") {
    const treatments = get("treatments").elements.map((t) => valueOf(t));
    return [
      literal("id"),
      {
        system: "legacy",
        kind: kind.value,
        label: literal("label"),
        default: valueOf(get("defaultConfig")),
        variants: Object.fromEntries(treatments.map((t) => [String(t?.id), { label: t?.label, config: t?.config }])),
      },
    ];
  }
  return null;
}

/** What a module's code refers to: its imports (`r = n(17928)`), the
 * experiments it defines, its exports (`n.d(t, { ew: () => o })`) and its
 * other declarations, each keyed by the identifier declaring it, as
 * minifiers reuse a name in scopes that don't see each other. */
function bindingsOf(body, require) {
  const declared = declarations(body);
  const imports = new Map();
  const experiments = new Map();
  const exported = new Map();
  const locals = new Map();
  walk(body, (n) => {
    if (n.type === "VariableDeclarator" && n.id.type === "Identifier" && n.init) {
      const init = n.init;
      const call = init.type === "CallExpression" ? init : null;
      if (
        call?.callee.type === "Identifier" &&
        call.callee.name === require &&
        typeof call.arguments[0]?.value === "number"
      ) {
        imports.set(n.id, String(call.arguments[0].value));
      } else if (call && experimentOf(call.arguments[0])) {
        experiments.set(n.id, experimentOf(call.arguments[0])[0]);
      } else {
        locals.set(n.id, init);
      }
    }
    if (n.type === "FunctionDeclaration" && n.id) locals.set(n.id, n.body);
    const callee = n.type === "CallExpression" ? n.callee : null;
    // `n.d(t, { K: () => a })` exports through getters; newer builds also
    // pass values, the code itself: `n.d(t, {}, { K: function () { … } })`.
    if (callee?.type === "MemberExpression" && callee.object.name === require && callee.property.name === "d") {
      const [, getters, values] = n.arguments;
      for (const p of getters?.type === "ObjectExpression" ? getters.properties : []) {
        const value = isFunction(p.value) ? p.value.body : null;
        if (value?.type === "Identifier" || value?.type === "MemberExpression") exported.set(keyName(p), value);
      }
      for (const p of values?.type === "ObjectExpression" ? values.properties : []) {
        if (p.type === "Property") exported.set(keyName(p), p.value);
      }
    }
  });
  // `let l = a;` re-exports an experiment under another name.
  for (const [id, init] of locals) {
    const target = init.type === "Identifier" ? declared.get(init) : null;
    if (experiments.has(target)) experiments.set(id, experiments.get(target));
  }
  return { declared, imports, experiments, exported, locals };
}

/** The experiments `node` uses directly, and the exports of other modules
 * it reads (`"219882.ew"`), following the module's own functions. */
function touches(node, bindings) {
  const experiments = new Set();
  const reads = new Set();
  const seen = new Set();
  const queue = [node];
  while (queue.length > 0) {
    const stack = [queue.pop()];
    while (stack.length > 0) {
      const n = stack.pop();
      if (n.type === "MemberExpression" && !n.computed) {
        const from = n.object.type === "Identifier" ? bindings.imports.get(bindings.declared.get(n.object)) : null;
        if (from) reads.add(`${from}.${n.property.name}`);
        else stack.push(n.object);
        continue;
      }
      if (n.type === "Identifier") {
        const id = bindings.declared.get(n);
        if (bindings.experiments.has(id)) experiments.add(bindings.experiments.get(id));
        else if (bindings.locals.has(id) && !seen.has(id)) {
          seen.add(id);
          queue.push(bindings.locals.get(id));
        }
        continue;
      }
      for (const key in n) {
        // Names that aren't references: object keys and method names.
        if (key === "key" && !n.computed && ["Property", "MethodDefinition", "PropertyDefinition"].includes(n.type))
          continue;
        const value = n[key];
        if (Array.isArray(value)) {
          for (const child of value) if (child && typeof child.type === "string") stack.push(child);
        } else if (value && typeof value.type === "string") stack.push(value);
      }
    }
  }
  return { experiments: [...experiments], reads: [...reads] };
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
  const stores = new Map(WATCH.stores.map((name) => [name, new Set()]));
  const constants = new Map(WATCH.constants.map((name) => [name, new Set()]));
  const experiments = new Map();
  // What each module's exports touch, and what the code holding a rule
  // Neucord copies touches: the watched methods and stores, and the modules
  // MANUAL.md names. Resolved across modules once every module is read.
  const exportsTouch = new Map();
  const copied = new Map();
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
    for (const { id, property } of modulesOf(ast)) {
      modules++;
      const body = property.value;
      const require = body.params[2]?.name;
      const bindings = bindingsOf(body, require);
      const exports = {};
      for (const [name, value] of bindings.exported) exports[name] = touches(value, bindings);
      exportsTouch.set(id, exports);
      // Keyed by the method's or store's name, which survive the module
      // renumbering a build can bring; MANUAL.md's modules only have ids.
      const copy = (scope, key) => {
        const found = touches(scope, bindings);
        const all = copied.get(key) ?? { experiments: new Set(), reads: new Set() };
        found.experiments.forEach((e) => all.experiments.add(e));
        found.reads.forEach((r) => all.reads.add(r));
        copied.set(key, all);
      };
      if (MANUAL.has(id)) copy(body, `module ${id}`);
      const text = source.slice(body.start, body.end);
      const isGatewayModule = text.includes('"READY_SUPPLEMENTAL"') && text.includes('"INITIAL_GUILD"');
      // Constants are named where they are used or exported, and often
      // defined apart: `MAX: () => c` in the export map, `c = 30` below it.
      const declared = new Map();
      const named = [];
      // A method exported by name from a function declared apart:
      // `getEmojiUnavailableReason: N` in the export map, `function N(e)` above.
      const functions = new Map();
      const aliased = [];
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
          const experiment = experimentOf(node.arguments[0]);
          if (experiment) experiments.set(experiment[0], JSON.stringify(sorted(experiment[1])));
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
        if (node.type === "FunctionDeclaration" && node.id) functions.set(node.id.name, node);
        if (node.type === "Property" && node.value.type === "Identifier" && methods.has(keyName(node))) {
          aliased.push([keyName(node), node.value.name]);
        }
        // Class methods and object methods alike: `isSystemDM() { … }`.
        if ((node.type === "MethodDefinition" || node.type === "Property") && isFunction(node.value)) {
          const name = !node.computed && node.key.type === "Identifier" ? node.key.name : null;
          if (methods.has(name)) {
            methods.get(name).add(normalized(source, node.value));
            copy(node.value, name);
          }
        }
        // Stores name themselves: `static displayName = "GuildVerificationStore"`.
        // Their rules live in functions of the module around them.
        if (node.type === "PropertyDefinition" && node.static && !node.computed && node.key.name === "displayName") {
          const name = node.value?.value;
          if (stores.has(name)) {
            stores.get(name).add(normalized(source, body, { module: true }));
          }
        }
        // A store's class: its rules are its methods and what they call.
        if (node.type === "ClassDeclaration" || node.type === "ClassExpression") {
          const store = node.body.body.find(
            (m) =>
              m.type === "PropertyDefinition" && m.static && m.key.name === "displayName" && stores.has(m.value?.value),
          );
          if (store) copy(node.body, store.value.value);
        }
        const table = node.type === "ObjectExpression" && objectEnum(node);
        if (table) {
          for (const [name, anchors] of Object.entries(WATCH.enums)) {
            if (anchors.every((a) => Object.hasOwn(table, a))) enums.get(name).add(JSON.stringify(sorted(table)));
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
      for (const [name, id] of aliased) {
        const target = functions.get(id);
        if (target) {
          methods.get(name).add(normalized(source, target));
          copy(target, name);
        }
      }
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

  // The experiments code holding a copied rule reads, itself or through an
  // export of the module defining them: any of them may change that rule
  // per account. Following helpers further mostly finds the media engine's.
  const resolve = (read, depth, seen) => {
    if (depth === 0 || seen.has(read)) return [];
    seen.add(read);
    const [module, name] = read.split(".");
    const table = exportsTouch.get(module);
    // Export names such as `constructor` would find the prototype's.
    if (!table || !Object.hasOwn(table, name)) return [];
    const touched = table[name];
    return [...touched.experiments, ...touched.reads.flatMap((r) => resolve(r, depth - 1, seen))];
  };
  const reached = {};
  for (const [key, { experiments: own, reads }] of copied) {
    const seen = new Set();
    const names = new Set([...own, ...[...reads].flatMap((r) => resolve(r, 1, seen))]);
    if (names.size > 0) reached[key] = [...names].sort();
  }

  return sorted({
    channel: meta.channel,
    build: meta.build,
    apiVersion: meta.apiVersion ?? null,
    stats: { scripts: meta.scripts, modules, unparsed },
    endpoints,
    calls: Object.fromEntries([...calls].map(([k, v]) => [k, [...v].sort().map((s) => JSON.parse(s))])),
    gateway: Object.fromEntries([...gateway].map(([k, v]) => [k, [...v].sort()])),
    headers: [...headers].sort(),
    enums: Object.fromEntries([...enums].map(([k, v]) => [k, [...v].sort().map((s) => JSON.parse(s))])),
    methods: Object.fromEntries([...methods].map(([k, v]) => [k, [...v].sort()])),
    stores: Object.fromEntries([...stores].map(([k, v]) => [k, [...v].sort()])),
    constants: Object.fromEntries([...constants].map(([k, v]) => [k, [...v].sort().map((s) => JSON.parse(s))])),
    experiments: Object.fromEntries([...experiments].map(([k, v]) => [k, JSON.parse(v)])),
    reached,
  });
}
