import {execFileSync} from 'node:child_process';
import {mkdir, readFile} from 'node:fs/promises';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import appleSilicon from '../platforms/macos-arm64/target.mjs';
import intel from '../platforms/macos-x64/target.mjs';

export function latestEditions(history) {
  return [[appleSilicon, 'Apple-Silicon'], [intel, 'Intel']].map(([target, edition]) => {
    const matches = history.filter(item => typeof item.name === 'string' && item.name.startsWith(`${target.artifact}-v`));
    matches.sort((a, b) => String(b.createdDate).localeCompare(String(a.createdDate)));
    return {edition, submission: matches[0] ?? null};
  });
}

export async function inspectNotarization({runNotary, directory, log = console.log}) {
  const history = JSON.parse(runNotary('history', '--output-format', 'json')).history;
  if (!Array.isArray(history)) throw new Error('Invalid notarization history');
  await mkdir(directory, {recursive: true});
  for (const {edition, submission} of latestEditions(history)) {
    if (!submission) { log(`No submission found for ${edition}.`); continue; }
    const {id, name, status, createdDate} = submission;
    log(JSON.stringify({id, name, status, createdDate}));
    if (!['Accepted', 'Invalid', 'Rejected'].includes(status)) continue;
    const destination = path.join(directory, `${edition}.json`);
    runNotary('log', id, destination);
    const report = JSON.parse(await readFile(destination, 'utf8'));
    const issues = report.issues ?? [];
    const counts = new Map();
    for (const issue of issues) counts.set(issue.message ?? '', (counts.get(issue.message ?? '') ?? 0) + 1);
    log(JSON.stringify({edition, status: report.status, issueCounts: Object.fromEntries(counts)}));
    for (const issue of issues.slice(0, 10)) log(JSON.stringify(issue));
  }
}

if (process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1])) {
  const names = ['APPLE_ID', 'APPLE_TEAM_ID', 'APPLE_APP_PASSWORD'];
  if (names.some(name => !process.env[name]) || !process.env.RUNNER_TEMP) throw new Error('Apple notarization credentials or RUNNER_TEMP are incomplete');
  const redact = text => names.reduce((result, name) => result.replaceAll(process.env[name], '[REDACTED]'), String(text));
  const auth = ['--apple-id', process.env.APPLE_ID, '--team-id', process.env.APPLE_TEAM_ID, '--password', process.env.APPLE_APP_PASSWORD];
  await inspectNotarization({
    directory: path.join(process.env.RUNNER_TEMP, 'notarization-reports'),
    runNotary(...args) {
      try { return execFileSync('xcrun', ['notarytool', ...args, ...auth], {encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe']}); }
      catch (error) { throw new Error(redact(error.stderr || 'Apple notarization inspection failed')); }
    },
    log: value => console.log(redact(value)),
  });
}
