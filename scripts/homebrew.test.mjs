import assert from 'node:assert/strict';
import test from 'node:test';
import { formulaFor, updateHomebrew } from './homebrew.mjs';

const names = ['darwin-arm64', 'darwin-amd64', 'linux-arm64', 'linux-amd64'];
const checksums = names.map((name, i) => `${String(i).repeat(64)}  aggr-${name}.tar.gz`).join('\n');
const fetchPublic = async (_url, options) => {
  assert.ok(options.signal);
  assert.equal(options.headers, undefined);
  return new Response(checksums);
};

test('formula installs release binaries for all four Homebrew platforms with checksums', async () => {
  const formula = await formulaFor('v2.0.0', fetchPublic);
  for (const [i, name] of names.entries()) {
    assert.ok(formula.includes(`/v2.0.0/aggr-${name}.tar.gz`));
    assert.ok(formula.includes(`sha256 "${String(i).repeat(64)}"`));
  }
  assert.ok(formula.includes('depends_on "git"'));
  assert.ok(formula.includes('bin.install "aggr"'));
  assert.ok(formula.includes('pkgshare.install "THIRD_PARTY_NOTICES"'));
});

test('invalid tags and incomplete or malformed checksums cannot update a formula', async () => {
  for (const tag of ['main', 'v2.0.0-rc.1', 'v2.0.0\n', 'v2.0.0"'])
    await assert.rejects(formulaFor(tag, fetchPublic));
  for (const body of ['', checksums.split('\n').slice(1).join('\n'), `${checksums}\n${checksums}`, 'invalid'])
    await assert.rejects(formulaFor('v2.0.0', async () => new Response(body)));
  await assert.rejects(formulaFor('v2.0.0', async () => new Response('', { status: 404 })));
});

function fixture(previous) {
  const calls = [], logs = [];
  return { calls, logs, options: {
    tag: 'v2.0.0', fetchPublic, log: message => logs.push(message),
    api: async (path, input) => {
      calls.push({ path, input });
      if (input) return {};
      if (path.endsWith('/Formula'))
        return previous ? [{ name: 'aggr.rb', sha: 'old-sha' }] : [];
      return { content: Buffer.from(previous).toString('base64'), sha: 'current-sha' };
    },
  } };
}

test('first publication creates only aggr.rb in the shared tap', async () => {
  const f = fixture();
  await updateHomebrew(f.options);
  assert.equal(f.calls.length, 2);
  assert.equal(f.calls[1].path, 'repos/aymericbeaumet/homebrew-tap/contents/Formula/aggr.rb');
  assert.equal(f.calls[1].input.sha, undefined);
  assert.equal(Buffer.from(f.calls[1].input.content, 'base64').toString(), await formulaFor('v2.0.0', fetchPublic));
});

test('updates use the current blob sha to preserve concurrent tap edits', async () => {
  const f = fixture('class Aggr < Formula\n  version "1.12.0"\nend\n');
  await updateHomebrew(f.options);
  assert.equal(f.calls[2].input.sha, 'current-sha');
});

test('an identical publication is a no-op', async () => {
  const f = fixture(await formulaFor('v2.0.0', fetchPublic));
  await updateHomebrew(f.options);
  assert.equal(f.calls.length, 2);
  assert.ok(f.logs[0].includes('already'));
});

test('old release retries and unknown existing formats cannot overwrite the tap', async () => {
  for (const previous of ['  version "2.1.0"', '  version "10.0.0"', 'unknown']) {
    const f = fixture(previous);
    await assert.rejects(updateHomebrew(f.options));
    assert.equal(f.calls.length, 2);
  }
});

test('a failed tap write surfaces instead of reporting publication', async () => {
  const f = fixture();
  const original = f.options.api;
  await assert.rejects(updateHomebrew({ ...f.options, api: async (...args) => {
    if (args[1]) throw new Error('conflicting edit');
    return original(...args);
  } }), /conflicting edit/);
  assert.deepEqual(f.logs, []);
});
