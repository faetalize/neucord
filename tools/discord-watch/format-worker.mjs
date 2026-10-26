// Formats module sources for unpack.mjs, off the main thread.
import { parentPort } from "node:worker_threads";
import { format } from "prettier";

parentPort.on("message", async ({ id, source }) => {
  let text = source;
  try {
    text = await format(source, { parser: "babel", printWidth: 120 });
  } catch {
    // Keep it unformatted; it still greps.
  }
  parentPort.postMessage({ id, text });
});
