import { writeFile } from 'node:fs/promises';

const repository = 'aymericbeaumet/aggr';
const tap = 'aymericbeaumet/homebrew-tap';
const platforms = ['darwin-arm64', 'darwin-amd64', 'linux-arm64', 'linux-amd64'];

export async function formulaFor(tag, fetchPublic = fetch) {
  if (typeof tag !== 'string' || tag !== tag.trim() || !/^v\d+\.\d+\.\d+$/.test(tag))
    throw new Error('Expected a stable release tag vX.Y.Z');
  const base = `https://github.com/${repository}/releases/download/${tag}`;
  const response = await fetchPublic(`${base}/SHA256SUMS`, { signal: AbortSignal.timeout(15_000) });
  if (!response.ok) throw new Error(`Reading release checksums failed (HTTP ${response.status})`);
  const sums = new Map();
  for (const line of (await response.text()).trim().split('\n')) {
    const match = /^([a-f0-9]{64}) [ *]([^/\s]+)$/.exec(line);
    if (!match || sums.has(match[2])) throw new Error('Malformed or duplicate release checksum');
    sums.set(match[2], match[1]);
  }
  const block = platform => {
    const archive = `aggr-${platform}.tar.gz`;
    if (!sums.has(archive)) throw new Error(`Missing checksum for ${archive}`);
    return `      url "${base}/${archive}"\n      sha256 "${sums.get(archive)}"`;
  };
  const blocks = platforms.map(block);
  return `class Aggr < Formula
  desc "Git-native feed reader: aggr.toml in, static site out"
  homepage "https://github.com/${repository}"
  version "${tag.slice(1)}"
  license "MIT"
  depends_on "git"

  on_macos do
    on_arm do
${blocks[0]}
    end
    on_intel do
${blocks[1]}
    end
  end

  on_linux do
    on_arm do
${blocks[2]}
    end
    on_intel do
${blocks[3]}
    end
  end

  def install
    bin.install "aggr"
    pkgshare.install "THIRD_PARTY_NOTICES"
  end

  test do
    assert_match "aggr #{version}", shell_output("#{bin}/aggr --version")
    assert_match "--native", shell_output("#{bin}/aggr dev --help")
    system "#{bin}/aggr", "init"
    assert_path_exists testpath/"aggr.toml"
  end
end
`;
}

export async function updateHomebrew({ tag, fetchPublic = fetch, api, log = console.log }) {
  const formula = await formulaFor(tag, fetchPublic);
  const listing = await api(`repos/${tap}/contents/Formula`);
  const entry = listing.find(entry => entry.name === 'aggr.rb');
  if (entry) {
    const current = await api(`repos/${tap}/contents/Formula/aggr.rb`);
    const previous = Buffer.from(current.content, 'base64').toString('utf8');
    if (previous === formula) { log(`${tag} is already in the Homebrew tap`); return; }
    const oldVersion = /^  version "(\d+\.\d+\.\d+)"$/m.exec(previous)?.[1];
    if (!oldVersion) throw new Error('Cannot identify the existing Homebrew version');
    const oldParts = oldVersion.split('.').map(BigInt), newParts = tag.slice(1).split('.').map(BigInt);
    const difference = newParts.findIndex((part, index) => part !== oldParts[index]);
    if (difference >= 0 && newParts[difference] < oldParts[difference])
      throw new Error(`Refusing to replace Homebrew ${oldVersion} with ${tag}`);
    entry.sha = current.sha;
  }
  await api(`repos/${tap}/contents/Formula/aggr.rb`, {
    message: `chore: update aggr to ${tag}`, content: Buffer.from(formula).toString('base64'),
    ...(entry ? { sha: entry.sha } : {}),
  });
  log(`Updated ${tap} to ${tag}`);
}

if (import.meta.main) {
  try {
    const [tag, option, output] = process.argv.slice(2);
    if (option === '--output' && output) await writeFile(output, await formulaFor(tag));
    else if (option || output) throw new Error('Usage: homebrew.mjs vX.Y.Z [--output PATH]');
    else {
      if (!process.env.GH_TOKEN) throw new Error('Set HOMEBREW_TAP_TOKEN as GH_TOKEN for the tap update');
      await updateHomebrew({ tag, api: async (path, input) => {
        const response = await fetch(`https://api.github.com/${path}`, {
          method: input ? 'PUT' : 'GET', body: input && JSON.stringify(input),
          signal: AbortSignal.timeout(30_000),
          headers: { Authorization: `Bearer ${process.env.GH_TOKEN}`, Accept: 'application/vnd.github+json',
            'Content-Type': 'application/json', 'X-GitHub-Api-Version': '2022-11-28' },
        });
        if (!response.ok) throw new Error(`GitHub tap request failed (HTTP ${response.status})`);
        return response.json();
      } });
    }
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}
