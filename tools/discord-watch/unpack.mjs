// Split a build into one readable file per webpack module, for reading and
// grepping Discord's client. Module ids are stable across builds, so the
// files of two builds diff module by module.

import { availableParallelism } from "node:os";
import { mkdir, readdir, readFile, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { Worker } from "node:worker_threads";
import { parse, modulesOf } from "./js.mjs";

// Modules that are only JSON (animations, mostly): kept as a stub.
const DATA_MODULE = /^\w*\(\w*(?:,\w*)*\)\{\w+\.exports=JSON\.parse\(/;

export async function unpack(dir, out, { log = () => {} } = {}) {
  const modulesDir = join(out, "modules");
  await rm(modulesDir, { recursive: true, force: true });
  await mkdir(modulesDir, { recursive: true });

  const jobs = [];
  for (const file of (await readdir(join(dir, "assets"))).sort()) {
    const source = await readFile(join(dir, "assets", file), "utf8");
    let ast;
    try {
      ast = parse(source);
    } catch {
      continue;
    }
    for (const { id, property } of modulesOf(ast)) {
      const text = source.slice(property.start, property.end);
      const header = `// Module ${id}, from ${file}\n`;
      if (text.length > 20000 && DATA_MODULE.test(text)) {
        jobs.push({ id, done: `${header}// JSON data, ${text.length} bytes; see the chunk.\n` });
      } else {
        jobs.push({ id, header, source: `({\n${text}\n});\n` });
      }
    }
  }
  log(`${jobs.length} modules`);

  const pending = jobs.filter((j) => !j.done).sort((a, b) => b.source.length - a.source.length);
  const results = new Map();
  const workers = Array.from(
    { length: Math.max(1, availableParallelism() - 1) },
    () => new Worker(new URL("./format-worker.mjs", import.meta.url)),
  );
  await Promise.all(
    workers.map(
      (worker) =>
        new Promise((resolve) => {
          const next = () => {
            const job = pending.shift();
            if (!job) return resolve(worker.terminate());
            worker.once("message", ({ id, text }) => {
              results.set(id, job.header + text);
              next();
            });
            worker.postMessage({ id: job.id, source: job.source });
          };
          next();
        }),
    ),
  );
  for (const job of jobs) await writeFile(join(modulesDir, `${job.id}.js`), job.done ?? results.get(job.id));
  return jobs.length;
}
