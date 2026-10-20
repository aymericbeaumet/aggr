import { spawn } from 'node:child_process';
import { copyFile, mkdir, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, dirname, join } from 'node:path';
import { createInterface } from 'node:readline';
import { performance } from 'node:perf_hooks';
import { buildArguments, workerArguments } from './context.mjs';

const readyPrefix = '\x1eaggr-ready ';

function exitOf(child) {
  return new Promise((resolve, reject) => {
    child.once('error', reject);
    child.once('exit', (code, signal) => resolve({ code, signal }));
  });
}

async function killTree(child) {
  if (!child.pid || child.exitCode !== null || child.signalCode !== null) return;
  if (process.platform === 'win32') {
    const killer = spawn('taskkill', ['/PID', String(child.pid), '/T', '/F'], { stdio: 'ignore' });
    await exitOf(killer).catch(() => child.kill());
  } else {
    try { process.kill(-child.pid, 'SIGKILL'); }
    catch (error) { if (error.code !== 'ESRCH') child.kill('SIGKILL'); }
  }
}

async function stopWorker(worker) {
  if (!worker) return;
  if (worker.child.exitCode === null && worker.child.signalCode === null) {
    worker.child.stdin.end('shutdown\n');
    let timer;
    const timeout = new Promise(resolve => { timer = setTimeout(() => resolve(false), 15000); });
    const stopped = await Promise.race([worker.exit.then(() => true), timeout]);
    clearTimeout(timer);
    if (!stopped) await killTree(worker.child);
  }
  await worker.exit;
}

export class Backend {
  #context;
  #hooks;
  #directory;
  #sequence = 0;
  #current;
  #compiler;
  #starting;
  #work;
  #state = { kind: 'idle' };

  constructor(context, hooks = {}) {
    this.#context = context;
    this.#hooks = hooks;
  }

  get origin() { return this.#current?.origin; }
  get base() { return this.#current?.base ?? '/'; }

  async start() {
    this.#directory = await mkdtemp(join(tmpdir(), 'aggr-dev-'));
    if (this.#state.kind === 'stopping') {
      await rm(this.#directory, { recursive: true, force: true });
      return;
    }
    const executable = await this.#copy(this.#context.executable);
    if (this.#state.kind === 'stopping') return;
    this.#current = await this.#launch(executable, false);
    this.#hooks.ready?.(this.#current);
  }

  rebuild() {
    if (this.#state.kind === 'stopping') return;
    if (this.#state.kind === 'building') { this.#state.again = true; return; }
    if (this.#state.kind === 'queued') clearTimeout(this.#state.timer);
    this.#state = { kind: 'queued', timer: setTimeout(() => this.#build(), 150) };
  }

  async #build() {
    const state = { kind: 'building', again: false };
    this.#state = state;
    this.#work = (async () => {
      const measurement = { kind: 'build', startedAt: new Date().toISOString(), status: 'failed', phase: 'compile', compileMs: 0, prepareMs: null };
      const compileStarted = performance.now();
      let prepareStarted;
      const prepared = () => {
        if (prepareStarted === undefined) return;
        measurement.prepareMs = performance.now() - prepareStarted;
        prepareStarted = undefined;
      };
      try {
        let artifact;
        try { artifact = await this.#compile(); }
        finally { measurement.compileMs = performance.now() - compileStarted; }
        if (!artifact || this.#state.kind === 'stopping') { measurement.status = 'cancelled'; return; }
        if (state.again) { measurement.status = 'superseded'; return; }
        measurement.phase = 'prepare';
        prepareStarted = performance.now();
        const executable = await this.#copy(artifact);
        if (state.again) {
          measurement.status = 'superseded';
          await rm(dirname(executable), { recursive: true, force: true });
          return;
        }
        const previous = this.#current;
        this.#current = undefined;
        await stopWorker(previous);
        if (this.#state.kind === 'stopping') { measurement.status = 'cancelled'; return; }
        try {
          this.#current = await this.#launch(executable, true);
        } catch (error) {
          prepared();
          if (previous && this.#state.kind !== 'stopping') {
            console.error('The new worker could not start; restoring the previous build.');
            this.#current = await this.#launch(previous.executable, true);
            this.#hooks.ready?.(this.#current);
          }
          await rm(dirname(executable), { recursive: true, force: true });
          throw error;
        }
        prepared();
        this.#hooks.ready?.(this.#current);
        measurement.status = 'success';
        if (previous) await rm(previous.directory, { recursive: true, force: true });
      } catch (error) {
        console.error(error instanceof Error ? error.message : error);
        this.#hooks.buildError?.(error);
      } finally {
        prepared();
        const preparation = measurement.prepareMs === null ? 'skipped' : `${(measurement.prepareMs / 1000).toFixed(2)}s`;
        console.log(`dev build: compile ${(measurement.compileMs / 1000).toFixed(2)}s, prepare ${preparation} (${measurement.status})`);
        try { this.#hooks.metrics?.(measurement); }
        catch (error) { console.error('Could not record development timings:', error); }
      }
    })();
    await this.#work;
    this.#work = undefined;
    if (this.#state.kind === 'stopping') return;
    this.#state = { kind: 'idle' };
    if (state.again) this.rebuild();
  }

  async #copy(source) {
    const directory = join(this.#directory, String(this.#sequence++));
    await mkdir(directory);
    const executable = join(directory, basename(source));
    await copyFile(source, executable);
    return executable;
  }

  async #compile() {
    console.log('Rebuilding Rust; the current reader stays available.');
    if (this.#hooks.compile) return this.#hooks.compile();
    const compiler = spawn('cargo', buildArguments(this.#context), {
      cwd: this.#context.repository,
      detached: process.platform !== 'win32',
      stdio: ['ignore', 'pipe', 'inherit'],
    });
    this.#compiler = compiler;
    const exit = exitOf(compiler);
    let artifact;
    const lines = createInterface({ input: compiler.stdout });
    lines.on('line', line => {
      let event;
      try { event = JSON.parse(line); } catch { console.log(line); return; }
      if (event.reason === 'compiler-message' && event.message?.rendered) process.stderr.write(event.message.rendered);
      if (event.reason === 'compiler-artifact' && event.target?.name === 'aggr-dev-worker' && event.executable) artifact = event.executable;
    });
    const result = await exit;
    this.#compiler = undefined;
    lines.close();
    if (this.#state.kind === 'stopping') return;
    if (result.code !== 0 || !artifact) throw new Error('Rust compilation failed; the previous reader is still running.');
    return artifact;
  }

  async #launch(executable, restart) {
    const environment = {
      ...process.env,
      AGGR_DEV_INTERNAL_WORKER: '1',
      AGGR_DEV_SUPERVISED: '1',
      AGGR_DEV_RESTART: restart ? '1' : '0',
      AGGR_DEV_PUBLIC_ORIGIN: this.#context.viteOrigin,
    };
    delete environment.AGGR_VITE_URL;
    const child = spawn(executable, workerArguments(this.#context, restart), {
      cwd: this.#context.cwd,
      env: environment,
      detached: process.platform !== 'win32',
      stdio: ['pipe', 'pipe', 'inherit'],
      windowsHide: true,
    });
    const exit = exitOf(child);
    this.#starting = { child, exit };
    const lines = createInterface({ input: child.stdout });
    let timer;
    const ready = new Promise((resolve, reject) => {
      timer = setTimeout(() => reject(new Error('The development worker did not become ready.')), 300000);
      lines.on('line', line => {
        if (!line.startsWith(readyPrefix)) { console.log(line); return; }
        try {
          const ready = JSON.parse(line.slice(readyPrefix.length));
          const origin = new URL(ready.origin);
          if (origin.protocol !== 'http:' || origin.hostname !== '127.0.0.1' || typeof ready.base !== 'string') throw new Error('Invalid worker readiness response.');
          resolve({ origin: origin.origin, base: ready.base });
        } catch (error) { reject(error); }
      });
      exit.then(result => reject(new Error(`Development worker exited before readiness (${result.code ?? result.signal}).`)), reject);
    });
    let info;
    try { info = await ready; }
    catch (error) { await killTree(child); await exit.catch(() => {}); throw error; }
    finally { clearTimeout(timer); this.#starting = undefined; }
    if (this.#state.kind === 'stopping') {
      await stopWorker({ child, exit });
      throw new Error('Development is stopping.');
    }
    const worker = { ...info, child, exit, executable, directory: dirname(executable) };
    exit.then(result => {
      lines.close();
      if (this.#current === worker && this.#state.kind !== 'stopping') {
        this.#current = undefined;
        this.#hooks.exit?.(new Error(`Development worker exited (${result.code ?? result.signal}).`));
      }
    }).catch(error => this.#hooks.exit?.(error));
    child.stdin.on('error', () => {});
    return worker;
  }

  async close() {
    if (this.#state.kind === 'queued') clearTimeout(this.#state.timer);
    this.#state = { kind: 'stopping' };
    if (this.#compiler) await killTree(this.#compiler);
    await stopWorker(this.#starting);
    await this.#work;
    await stopWorker(this.#current);
    this.#current = undefined;
    if (this.#directory) await rm(this.#directory, { recursive: true, force: true });
  }
}
