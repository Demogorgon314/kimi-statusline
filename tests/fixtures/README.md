These neutral wire fixtures were generated from the local Kimi Code checkout
at commit `21406fb4c`, using its `GoalCreate`, `GoalUpdate`, `UsageRecord` and
`ContextAppendLoopEvent` classes and `Event2.serialize()`. They are generated
records, not captures of private conversations. `wire-v1.5.jsonl` applies the
upstream `wire/migration/v1.5.ts` migration to `wire-v1.4.jsonl`.

Regenerate with Node 24 and an upstream checkout with dependencies installed:

```sh
node tests/fixtures/generate.mjs /path/to/kimi-code
```

For a checkout without dependencies, a third argument can point to an existing
Zod package directory. The generator does not edit the upstream checkout.
Cargo tests consume the checked-in files without Node or upstream dependencies.
