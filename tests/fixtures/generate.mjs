// Node >=24. Generate neutral fixtures with upstream event classes/serializer
// and its 1.4 -> 1.5 migration, without changing the upstream checkout.
// node tests/fixtures/generate.mjs /path/to/kimi-code [path/to/zod/package]
import { readFileSync, writeFileSync, existsSync } from 'node:fs';
import { resolve, dirname, join } from 'node:path';
import { pathToFileURL, fileURLToPath } from 'node:url';
import { registerHooks, stripTypeScriptTypes, createRequire } from 'node:module';

const checkout = resolve(process.argv[2]);
const source = join(checkout, 'packages/agent-core-v2/src');
const require = createRequire(pathToFileURL(join(checkout, 'packages/agent-core-v2/package.json')));
const zod = process.argv[3] ? resolve(process.argv[3], 'index.js') : require.resolve('zod');
registerHooks({
  resolve(specifier, context, next) {
    if (specifier === 'zod') return next(pathToFileURL(zod).href, context);
    if (specifier.startsWith('#/')) specifier = pathToFileURL(join(source, specifier.slice(2))).href;
    if (specifier.startsWith('.') || specifier.startsWith('file:')) {
      const url = new URL(specifier, context.parentURL);
      if (existsSync(fileURLToPath(url) + '.ts')) url.pathname += '.ts';
      return next(url.href, context);
    }
    return next(specifier, context);
  },
  load(url, context, next) {
    if (url.endsWith('.ts')) {
      return { format: 'module', source: stripTypeScriptTypes(readFileSync(new URL(url), 'utf8'),
        { mode: 'transform', sourceUrl: url }), shortCircuit: true };
    }
    return next(url, context);
  },
});

const upstream = rel => import(pathToFileURL(join(source, rel)).href);
const { GoalCreate, GoalUpdate } = await upstream('features/goal/goalOps.ts');
const { UsageRecord } = await upstream('agent/usage/usageOps.ts');
const { ContextAppendLoopEvent } = await upstream('agent/contextMemory/contextEvents.ts');
const { migrateV1_4ToV1_5 } = await upstream('wire/migration/v1.5.ts');
const agentId = 'main';
const usage = { inputOther: 100, output: 420, inputCacheRead: 900, inputCacheCreation: 0 };
const records = [
  { type: 'metadata', protocol_version: '1.4', created_at: 1000 },
  new GoalCreate({ agentId, goalId: 'fixture-goal', objective: 'Verify the status line' }, 1000).serialize(),
  new GoalUpdate({ agentId, budgetLimits: { turnBudget: 8 } }, 2000).serialize(),
  new UsageRecord({ agentId, model: 'kimi-code/k3', usage, usageScope: 'turn' }, 15000).serialize(),
  new ContextAppendLoopEvent({ agentId, event: { type: 'step.end', uuid: 'fixture-step', step: 1,
    finishReason: 'tool_use', usage, llmStreamDurationMs: 10000, llmFirstTokenLatencyMs: 5000 } }, 25000).serialize(),
  new GoalUpdate({ agentId, turnsUsed: 1, wallClockMs: 24000 }, 25000).serialize(),
  new GoalUpdate({ agentId, status: 'paused', wallClockMs: 30000 }, 31000).serialize(),
  new GoalUpdate({ agentId, status: 'active' }, 41000).serialize(),
];
const output = dirname(fileURLToPath(import.meta.url));
const write = (name, rows) => writeFileSync(join(output, name), rows.map(r => JSON.stringify(r)).join('\n') + '\n');
write('wire-v1.4.jsonl', records);
write('wire-v1.5.jsonl', records.map(record => record.type === 'metadata'
  ? { ...record, protocol_version: '1.5' } : migrateV1_4ToV1_5.migrateRecord(record)));
