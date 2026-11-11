// The client's Unicode emoji names as a table: which `:name:` writes which
// emoji, in the client's order and groups. The client keeps them in one JSON
// module (`nameToEmoji`, `emojis`, `emojisByCategory`); the table keeps the
// facts Neucord needs and none of the client's code.
//
//   node emoji-table.mjs <build dir | chunk.js>  >  crates/discord/src/emoji_names.txt
//
// The build dir is what `cli.mjs fetch` makes; the weekly snapshot hashes the
// same table (`emojiTable`) and says when Neucord's copy is out of date.
//
// A line is a group (`@people`; `@modifiers` holds names that only convert),
// an emoji (`😀 grinning grinning_face`: the emoji, then its names, the first
// being its own), or a skin tone variant of the emoji above it, indented
// (`\t👍🏻 1 thumbsup_tone1 …`: the emoji, the tones as 1 to 5, joined by `.`
// for two, then its names).

import { createHash } from "node:crypto";
import { readdir, readFile, stat } from "node:fs/promises";
import { join } from "node:path";
import { parse, walk } from "./js.mjs";

/** The client's emoji data in a chunk's source, or null. It is a module that
 * is nothing but `JSON.parse('…')`. */
export function emojiData(source) {
  if (!source.includes('"nameToEmoji"')) return null;
  let found = null;
  walk(parse(source), (node) => {
    const json =
      node.type === "CallExpression" &&
      node.callee.type === "MemberExpression" &&
      node.callee.object.name === "JSON" &&
      node.callee.property.name === "parse" &&
      node.arguments[0]?.type === "Literal" &&
      typeof node.arguments[0].value === "string" &&
      node.arguments[0].value.startsWith('{"emojis":');
    if (json) found = JSON.parse(node.arguments[0].value);
  });
  return found;
}

const TONES = ["1f3fb", "1f3fc", "1f3fd", "1f3fe", "1f3ff"];

/** The table for the client's data. */
export function emojiTable(data) {
  const lines = [];
  const children = new Set();
  const categories = Object.entries(data.emojisByCategory).sort((a, b) => a[1][0] - b[1][0]);
  for (const [category, [from, to]] of categories) {
    lines.push(`@${category}`);
    for (const emoji of data.emojis.slice(from, to)) {
      lines.push([emoji.surrogates, ...emoji.names].join(" "));
      for (const index of emoji.diversityChildren ?? []) {
        const child = data.emojis[index];
        children.add(index);
        const tones = child.diversity.map((code) => TONES.indexOf(code) + 1).join(".");
        lines.push(`\t${[child.surrogates, tones, ...child.names].join(" ")}`);
      }
    }
  }
  // The skin tone swatches themselves have names but no group or parent.
  const grouped = categories.at(-1)[1][1];
  const loose = data.emojis.filter((_, i) => i >= grouped && !children.has(i));
  if (loose.some((emoji) => !emoji.hasDiversityParent || emoji.diversity)) {
    throw new Error("an emoji belongs to no group and no emoji");
  }
  lines.push("@modifiers", ...loose.map((emoji) => [emoji.surrogates, ...emoji.names].join(" ")));
  return lines.join("\n") + "\n";
}

export const tableHash = (table) => createHash("sha256").update(table).digest("hex");

/** The table for a build directory or a chunk, or null if it has no data. */
export async function tableFor(path) {
  const files = (await stat(path)).isDirectory()
    ? (await readdir(join(path, "assets"))).sort().map((f) => join(path, "assets", f))
    : [path];
  for (const file of files) {
    const data = emojiData(await readFile(file, "utf8"));
    if (data) return emojiTable(data);
  }
  return null;
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const table = process.argv[2] && (await tableFor(process.argv[2]));
  if (!table) {
    console.error("usage: emoji-table.mjs <build dir | chunk.js>, holding the client's emoji data");
    process.exit(1);
  }
  process.stdout.write(table);
}
