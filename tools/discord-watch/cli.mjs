#!/usr/bin/env node
// discord-watch: follow changes in Discord's official client that matter to
// Neucord. See README.md.

import { execFileSync, spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { copyFile, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { currentBuild, fetchBuild } from "./fetch.mjs";
import { snapshot } from "./snapshot.mjs";
import { compare, report, unmatched } from "./diff.mjs";
import { neucordUses } from "./uses.mjs";
import { unpack } from "./unpack.mjs";

const USAGE = `usage:
  cli.mjs fetch <dir> [--channel stable|ptb|canary]   download a build
  cli.mjs snapshot <dir> [--out <file>]                distil it to JSON
  cli.mjs compare <old.json> <new.json> [--repo <neucord>] [--report <file>]
                                                       print {"affects", "other"} counts
  cli.mjs unpack <dir> <out>                           one formatted file per module
  cli.mjs update <decomp> [--channel …] [--force]      fetch, unpack and commit into a git repo
  cli.mjs account <decomp> <neucord>                   run Neucord logged in until READY, commit what
                                                       it was sent about experiments into <decomp>/account`;

const log = (message) => console.error(message);
const { positionals, values } = parseArgs({
  allowPositionals: true,
  options: {
    channel: { type: "string", default: "stable" },
    out: { type: "string" },
    repo: { type: "string", default: resolve(import.meta.dirname, "../..") },
    report: { type: "string" },
    force: { type: "boolean", default: false },
  },
});
const [command, ...args] = positionals;
const json = (value) => JSON.stringify(value, null, 2) + "\n";

switch (command) {
  case "fetch":
    console.log(json(await fetchBuild(args[0], { channel: values.channel, log })));
    break;
  case "snapshot": {
    const text = json(await snapshot(args[0]));
    if (values.out) await writeFile(values.out, text);
    else process.stdout.write(text);
    break;
  }
  case "compare": {
    const [old, cur] = await Promise.all(args.slice(0, 2).map(async (f) => JSON.parse(await readFile(f, "utf8"))));
    const uses = await neucordUses(values.repo);
    if (uses.unread.length > 0) {
      log("Endpoints discord-watch cannot read, so they would go unwatched:");
      for (const line of uses.unread) log(`  ${line}`);
      process.exit(1);
    }
    for (const { method, path } of unmatched(cur, uses)) log(`no client endpoint matches ${method} ${path}`);
    for (const [name, bodies] of Object.entries(cur.methods ?? {})) {
      if (bodies.length === 0) log(`no method ${name} in the client`);
    }
    for (const [name, values] of Object.entries(cur.constants ?? {})) {
      if (values.length === 0) log(`no constant ${name} in the client`);
    }
    for (const name of uses.experiments) {
      if (!cur.experiments?.[name]) log(`no experiment ${name} in the client`);
    }
    const result = compare(old, cur, uses);
    if (values.report) await writeFile(values.report, report(old, cur, result));
    console.log(JSON.stringify({ affects: result.affects.length, other: result.other.length }));
    break;
  }
  case "unpack":
    log(`${await unpack(args[0], args[1], { log })} modules written`);
    break;
  case "update": {
    const decomp = resolve(args[0]);
    const build = await currentBuild(values.channel);
    const stamp = join(decomp, "BUILD");
    const last = existsSync(stamp) ? (await readFile(stamp, "utf8")).trim() : "";
    if (last === `${values.channel} ${build}` && !values.force) {
      log(`already at ${last}`);
      break;
    }
    const dir = await mkdtemp(join(tmpdir(), "discord-watch-"));
    try {
      await fetchBuild(dir, { channel: values.channel, log });
      await unpack(dir, decomp, { log });
      await writeFile(join(decomp, "snapshot.json"), json(await snapshot(dir)));
      await writeFile(stamp, `${values.channel} ${build}\n`);
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
    const git = (...a) =>
      execFileSync("git", ["-C", decomp, ...a], { stdio: ["ignore", "pipe", "inherit"] }).toString();
    if (!existsSync(join(decomp, ".git"))) git("init", "-q");
    git("add", "-A");
    if (git("status", "--porcelain").trim()) git("commit", "-q", "-m", `${values.channel} build ${build}`);
    log(`${decomp} is at ${values.channel} build ${build}`);
    break;
  }
  case "account": {
    // What Discord sends this account, which no build shows: experiment
    // assignments and rollouts, named from the decomp's build, and READY's
    // shape. Neucord writes them on READY (NEUCORD_DEBUG_EXPERIMENTS) and is
    // stopped right after. Kept in the decomp, which never gets a remote.
    const decomp = resolve(args[0]);
    const snapshot = JSON.parse(await readFile(join(decomp, "snapshot.json"), "utf8"));
    if (!snapshot.experiments) throw new Error(`${decomp}/snapshot.json has no experiments; run update first`);
    const dir = await mkdtemp(join(tmpdir(), "discord-watch-account-"));
    try {
      await writeFile(join(dir, "names.txt"), Object.keys(snapshot.experiments).join("\n") + "\n");
      const neucord = spawn(resolve(args[1]), [], {
        env: { ...process.env, NEUCORD_DEBUG_EXPERIMENTS: dir },
        stdio: ["ignore", "ignore", "inherit"],
      });
      const exited = new Promise((done) => neucord.once("exit", done));
      const deadline = Date.now() + 120_000;
      // The shape is written last.
      while (!existsSync(join(dir, "ready-shape.txt")) && Date.now() < deadline && neucord.exitCode === null) {
        await new Promise((wait) => setTimeout(wait, 500));
      }
      neucord.kill();
      await exited;
      if (!existsSync(join(dir, "ready-shape.txt"))) {
        throw new Error("no READY: Neucord must be logged in, and not already running");
      }
      await mkdir(join(decomp, "account"), { recursive: true });
      for (const file of ["experiments.txt", "ready-shape.txt"])
        await copyFile(join(dir, file), join(decomp, "account", file));
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
    const git = (...a) =>
      execFileSync("git", ["-C", decomp, ...a], { stdio: ["ignore", "pipe", "inherit"] }).toString();
    git("add", "account");
    // What moved since the last check, for whoever runs it to report.
    process.stdout.write(git("diff", "--cached", "--", "account"));
    if (git("status", "--porcelain", "--", "account").trim()) {
      git(
        "commit",
        "-q",
        "-m",
        `account, ${new Date().toISOString().slice(0, 10)}, ${snapshot.channel} build ${snapshot.build}`,
      );
    }
    log(`${decomp}/account is up to date`);
    break;
  }
  default:
    console.error(USAGE);
    process.exit(command ? 1 : 0);
}
