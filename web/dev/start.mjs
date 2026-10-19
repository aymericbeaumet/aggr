import { join } from 'node:path';
import concurrently from 'concurrently';
import { createServer } from 'vite';
import { commands, readContext } from './context.mjs';

const context = readContext();
const origin = new URL(context.viteOrigin);
const frontend = await createServer({
  root: join(context.repository, 'web'),
  server: {
    host: origin.hostname.replace(/^\[|\]$/g, ''),
    port: Number(origin.port || 80),
    strictPort: context.strictPort ?? false,
  },
});
try {
  await frontend.listen();
  const address = frontend.httpServer.address();
  origin.port = String(address.port);
  frontend.config.server.origin = origin.origin;
  frontend.printUrls();
  const { result } = concurrently(commands({ ...context, viteOrigin: origin.origin }), {
    cwd: join(context.repository, 'web'),
    killSignal: 'SIGINT',
    killTimeout: 15000,
    prefix: 'name',
  });
  await result;
} catch (error) {
  console.error(error instanceof Error ? error.message : 'Development backend exited with an error.');
  process.exitCode = 1;
} finally {
  await frontend.close();
}
