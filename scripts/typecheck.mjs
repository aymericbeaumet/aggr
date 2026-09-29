import { mkdtempSync, readFileSync, rmSync, watch, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const watching = process.argv.includes("--watch");
const workerOnly = process.argv.includes("--worker-only");
const directory = mkdtempSync(join(tmpdir(), "aggr-typecheck-"));
const template = join(root, "themes/default/templates/sw.js");
const config = join(directory, "jsconfig.json");
const children = new Set();
const subscriptions = [];

function cleanup() {
  for (const child of children) child.kill();
  for (const subscription of subscriptions) subscription.close();
  rmSync(directory, { recursive: true, force: true });
}
process.on("exit", cleanup);
for (const signal of ["SIGINT", "SIGTERM"])
  process.on(signal, () => process.exit(signal === "SIGINT" ? 130 : 143));

function renderWorker() {
  const values = {
    version: "version", "build.app_version": "application", "build.content_version": "content",
    precache: [], offline_count: 0, offline_catalog: [],
    search_manifest: { version: "a".repeat(64), base: "pagefind/" + "a".repeat(64) + "/" },
  };
  const source = readFileSync(template, "utf8").replace(/\{\{\s*([\w.]+)\s*\|\s*json\s*\}\}/g, (_, name) => {
    if (!Object.hasOwn(values, name)) throw new Error("Unknown service-worker template value: " + name);
    return JSON.stringify(values[name]);
  });
  if (/\{[{%]/.test(source)) throw new Error("Unresolved service-worker template expression");
  // A service worker has a narrower global scope than TypeScript's shared WebWorker library.
  writeFileSync(join(directory, "worker.js"), "/** @param {ServiceWorkerGlobalScope} self */ function checkWorker(self) {\n" + source + "\n}\n");
}

writeFileSync(config, JSON.stringify({
  extends: join(root, "jsconfig.worker.json"), files: ["worker.js"], include: [],
}));
renderWorker();

function check(project) {
  return new Promise((resolve, reject) => {
    const args = ["--project", project, "--pretty", "false"];
    if (watching) args.push("--watch", "--preserveWatchOutput");
    const child = spawn("tsc", args, { cwd: root, shell: process.platform === "win32", stdio: ["ignore", "pipe", "pipe"] });
    children.add(child);
    for (const stream of [child.stdout, child.stderr]) {
      createInterface({ input: stream }).on("line", line => {
        process.stdout.write(line.replace(/^.*worker\.js\((\d+),(\d+)\)/, (_, row, column) =>
          `themes/default/templates/sw.js(${Number(row) - 1},${column})`) + "\n");
      });
    }
    child.on("error", reject);
    child.on("exit", code => { children.delete(child); resolve(code ?? 1); });
  });
}

if (watching) {
  subscriptions.push(watch(join(root, "themes/default/templates"), (_, name) => {
    if (name === "sw.js") {
      try { renderWorker(); }
      catch (error) { console.error(error.message); }
    }
  }));
}
const results = await Promise.all([...(workerOnly ? [] : [check(join(root, "jsconfig.json"))]), check(config)]);
process.exitCode = results.some(code => code !== 0) ? 1 : 0;
