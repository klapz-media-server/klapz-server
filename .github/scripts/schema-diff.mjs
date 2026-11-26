// Compares two SDL files and prints a Markdown report of what changed, by criticality.
//
// Usage: node schema-diff.mjs <base.graphql> <head.graphql>
// Needs `graphql` and `@graphql-inspector/core` (installed by the workflow).

import { readFileSync } from 'node:fs';
import { buildSchema } from 'graphql';
import { diff, CriticalityLevel } from '@graphql-inspector/core';

const [basePath, headPath] = process.argv.slice(2);
const load = (path) => buildSchema(readFileSync(path, 'utf8'));

const changes = await diff(load(basePath), load(headPath));

const groups = [
  [CriticalityLevel.Breaking, '🔴 Breaking'],
  [CriticalityLevel.Dangerous, '🟠 Dangerous'],
  [CriticalityLevel.NonBreaking, '🟢 Safe'],
];

const lines = [];
if (changes.length === 0) {
  lines.push('No change to the GraphQL schema.');
} else {
  const counts = groups
    .map(([level, label]) => [label, changes.filter((c) => c.criticality.level === level).length])
    .filter(([, n]) => n > 0)
    .map(([label, n]) => `${label}: **${n}**`);
  lines.push(counts.join(' · '), '');
  for (const [level, label] of groups) {
    const group = changes.filter((c) => c.criticality.level === level);
    if (group.length === 0) continue;
    // Safe changes are the bulk of most diffs and need no attention: folded away.
    const safe = level === CriticalityLevel.NonBreaking;
    lines.push(safe ? `<details><summary>${label} (${group.length})</summary>` : `#### ${label}`, '');
    for (const change of group) {
      const reason = !safe && change.criticality.reason ? ` — _${change.criticality.reason}_` : '';
      lines.push(`- ${change.message}${reason}`);
    }
    lines.push('', ...(safe ? ['</details>', ''] : []));
  }
}

process.stdout.write(lines.join('\n') + '\n');
// For the workflow: whether anything is breaking.
if (process.env.GITHUB_OUTPUT) {
  const breaking = changes.some((c) => c.criticality.level === CriticalityLevel.Breaking);
  const { appendFileSync } = await import('node:fs');
  appendFileSync(process.env.GITHUB_OUTPUT, `changes=${changes.length}\nbreaking=${breaking}\n`);
}
