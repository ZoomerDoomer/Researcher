# Real Bitcoin Core smoke test

This is the first step that requires access to a real Bitcoin Core node.

## Preconditions

The node must:

- be on the intended network;
- have completed Initial Block Download;
- be non-pruned so blocks are available from Genesis;
- expose JSON-RPC locally or over a trusted connection.

The `doctor` command checks the first three conditions before any research database is mutated.

## 1. Node preflight

Cookie auth example:

```bash
cargo run --locked -p researcher -- doctor \
  --cookie-file /path/to/.cookie
```

Expected final line:

```text
sync_ready=true
```

Do not continue if the command reports a network mismatch, Initial Block Download, or pruning.

## 2. First bounded scan

Use a fresh database and stop at block 1,000:

```bash
cargo run --locked -p researcher -- sync \
  --cookie-file /path/to/.cookie \
  --target-height 1000 \
  --db smoke.redb
```

Acceptance:

- exit code 0;
- `connected=1001` for a fresh database because Genesis is height 0;
- `disconnected=0`;
- final tip height is 1000.

## 3. Verify persisted state

Close the previous command completely, then run:

```bash
cargo run --locked -p researcher -- status --db smoke.redb
```

Acceptance:

- tip height remains 1000 after reopening the database;
- the printed tip hash is non-zero.

## 4. Resume instead of rebuilding

Continue the same database to block 5,000:

```bash
cargo run --locked -p researcher -- sync \
  --cookie-file /path/to/.cookie \
  --target-height 5000 \
  --db smoke.redb
```

Acceptance:

- `connected=4000`;
- `disconnected=0`;
- final tip height is 5000;
- the run resumes from 1001 rather than rescanning Genesis.

## 5. What to report back

Provide the complete console output of:

1. `doctor`
2. the height-1000 sync
3. `status`
4. the height-5000 resume

Do not start a full mainnet scan yet. The smoke output is used to decide whether RPC throughput, database size, or another real-node behavior requires adjustment first.
