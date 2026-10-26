// Reading Discord's bundles without running them: everything here works on
// acorn's syntax tree, never by evaluating Discord's code.

import * as acorn from "acorn";

export function parse(source) {
  return acorn.parse(source, { ecmaVersion: "latest", sourceType: "script", allowHashBang: true });
}

/** Visit every node, depth first. Return `false` from `visit` to skip a
 * node's children. Iterative: minified code nests deeper than the stack. */
export function walk(root, visit) {
  const stack = [root];
  while (stack.length > 0) {
    const node = stack.pop();
    if (visit(node) === false) continue;
    const children = [];
    for (const key in node) {
      const value = node[key];
      if (Array.isArray(value)) {
        for (const child of value) if (child && typeof child.type === "string") children.push(child);
      } else if (value && typeof value.type === "string") {
        children.push(value);
      }
    }
    for (let i = children.length - 1; i >= 0; i--) stack.push(children[i]);
  }
}

export const isFunction = (node) => node?.type === "FunctionExpression" || node?.type === "ArrowFunctionExpression";

/** A property's key as a string, or null for computed keys. */
export function keyName(property) {
  if (property.type !== "Property" || property.computed) return null;
  if (property.key.type === "Identifier") return property.key.name;
  if (property.key.type === "Literal") return String(property.key.value);
  return null;
}

/** Webpack module tables: object literals mapping numeric module ids to
 * module functions. Returns `{id, property}` for every module in a chunk. */
export function modulesOf(ast) {
  const modules = [];
  walk(ast, (node) => {
    if (node.type !== "ObjectExpression" || node.properties.length === 0) return;
    const isTable = node.properties.every(
      (p) => p.type === "Property" && /^\d+$/.test(keyName(p) ?? "") && isFunction(p.value),
    );
    if (!isTable) return;
    for (const property of node.properties) modules.push({ id: keyName(property), property });
    return false;
  });
  return modules;
}

/** Evaluate a constant expression built from literals, `+`, `===`, `?:`,
 * templates and object lookups, with `env` binding identifiers. Throws on
 * anything else. Used for the chunk-URL function, which is pure data. */
export function evaluate(node, env) {
  switch (node.type) {
    case "Literal":
      return node.value;
    case "Identifier":
      if (Object.hasOwn(env, node.name)) return env[node.name];
      break;
    case "TemplateLiteral":
      return node.quasis
        .map((q, i) => q.value.cooked + (i < node.expressions.length ? evaluate(node.expressions[i], env) : ""))
        .join("");
    case "BinaryExpression": {
      const [l, r] = [evaluate(node.left, env), evaluate(node.right, env)];
      if (node.operator === "+") return l + r;
      if (node.operator === "===" || node.operator === "==") return String(l) === String(r);
      break;
    }
    case "ConditionalExpression":
      return evaluate(node.test, env) ? evaluate(node.consequent, env) : evaluate(node.alternate, env);
    case "MemberExpression": {
      if (node.object.type !== "ObjectExpression") break;
      const key = String(node.computed ? evaluate(node.property, env) : node.property.name);
      const found = node.object.properties.find((p) => keyName(p) === key);
      return found ? evaluate(found.value, env) : undefined;
    }
  }
  throw new Error(`cannot evaluate ${node.type}`);
}
