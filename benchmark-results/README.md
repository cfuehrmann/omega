# Benchmark Results Store

Accumulated results from running Omega against Terminal-Bench 2.0.

## Files

| File | Purpose |
|---|---|
| `oracle-tasks.json` | All 89 TB2 tasks with category, difficulty, timeout, oracle status. 76 pass the oracle; 13 are excluded (GPU, huge downloads, or multi-hour builds). |
| `results.jsonl` | One JSON line per completed trial. Append-only. Deduplication key: `trial_id`. |
| `.skip-trials` | Trial IDs to permanently exclude from ingest (early infrastructure failures, not performance data). |

## Workflow

```bash
# 1. Run one task or a small batch
harbor run -d terminal-bench@2.0 \
  --agent-import-path omega_agent:OmegaAgent \
  -m anthropic/claude-sonnet-4-6 \
  --ae ANTHROPIC_API_KEY=$ANTHROPIC_API_KEY \
  -t terminal-bench/fix-git -n 1

# 2. Ingest results (idempotent — safe to re-run)
bun scripts/bench-ingest.ts
# or for a specific job directory:
bun scripts/bench-ingest.ts jobs/2026-05-01__10-00-00

# 3. View current state
bun scripts/bench-summary.ts
# filter by model:
bun scripts/bench-summary.ts sonnet
```

## results.jsonl schema

```jsonc
{
  "trial_id":        "uuid",           // Harbor trial UUID — dedup key
  "job_id":          "uuid | null",
  "task_name":       "terminal-bench/fix-git",
  "ingested_at":     "ISO-8601",
  "started_at":      "ISO-8601 | null",
  "finished_at":     "ISO-8601 | null",
  "runtime_sec":     908,              // wall-clock seconds for the whole trial
  "agent":           "omega",
  "model":           "claude-sonnet-4-6",
  "reward":          1.0,              // 0.0 or 1.0 (null if verifier never ran)
  "n_input_tokens":  50,               // non-cached input tokens
  "n_output_tokens": 5570,
  "n_cache_tokens":  540233,           // cache-read tokens (most of the cost)
  "exception":       null              // exception_type string if the run errored
}
```

Cost note: with Sonnet 4.6 pricing ($3/$15/$0.30 per MTok for input/output/cache), the
crack-7z-hash smoke test cost roughly $0.25.  Extrapolating: all 76 tasks ≈ $19.

## Oracle task status

13 tasks are marked `oracle_passes: false` in `oracle-tasks.json`:

| Task | Reason |
|---|---|
| build-pov-ray | Build takes many hours |
| caffe-cifar-10 | Requires GPU |
| compile-compcert | Very long compile (~40 min) |
| hf-model-inference | Large model download / GPU |
| install-windows-3.11 | Extremely long install |
| llm-inference-batching-scheduler | Requires GPU |
| pytorch-model-cli | Requires GPU |
| pytorch-model-recovery | Requires GPU |
| reshard-c4-data | C4 dataset is 100s of GB |
| sam-cell-seg | Requires GPU |
| torch-pipeline-parallelism | Requires GPU |
| torch-tensor-parallelism | Requires GPU |
| train-fasttext | Long training run |

These 13 were identified by cross-referencing agent timeouts, task categories, and
the oracle sweep count of 76/89 from session #1 (2026-04-23).  If a future oracle
re-run reveals a different list, update `oracle-tasks.json` accordingly.
