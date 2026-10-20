# LongMemEval — Bricks memory head-to-head

Runs the 500-question [LongMemEval benchmark](https://arxiv.org/abs/2410.10813)
(ICLR 2025) against four memory configurations built on Bricks primitives, so
we can publish numbers line-for-line comparable to Mastra's
[*Observational Memory*](https://mastra.ai/research/observational-memory)
research, the Zep Graphiti paper, and Supermemory's claims.

## Quick start

```bash
# 1. Download the datasets (oracle: 15 MB, small: 265 MB, medium: 2.6 GB)
./setup.sh                     # pulls all three variants; ~1–2 min on LFS
./setup.sh oracle              # or one at a time
./setup.sh s

# 2. Export the key of the embedding provider (default: gemini), read from the environment
export GOOGLE_API_KEY=...      # or GEMINI_API_KEY

# 3. Smoke: 10 questions × 4 configs on the small oracle set (~30 seconds)
cargo run --release -p longmem-bench -- \
  --dataset oracle --config all --limit 10 \
  --answerer-model gemini-2.5-flash --judge-model gemini-2.5-flash

# 4. Full run: 500 questions × 4 configs on longmemeval_s
cargo run --release -p longmem-bench -- \
  --dataset s --config all --concurrency 8 \
  --answerer-model gemini-2.5-flash --judge-model gemini-2.5-flash \
  --extractor-model gemini-2.5-flash
```

`--answerer-model` and `--judge-model` are required. `--extractor-model` is used by the hybrid configuration. `--embeddings` (default `gemini`) selects the embedding provider, and `--providers` names a providers file. Run `cargo run -p longmem-bench -- --help` for the full list.

Outputs land in `./results/` as `<config>-<dataset>.json` summaries plus
`<config>-rows-<dataset>.json` per-question traces, and one roll-up
`summary-<dataset>.json`.

## Configurations

| Config | Backend | What it tests |
|---|---|---|
| **A. `baseline`** | `JsonlMemory` full haystack in prompt | Control lower bound — how well does the answerer do with perfect recall? |
| **B. `embed`** | `EmbeddingMemory` (usearch HNSW, cosine) | Pure semantic retrieval. Direct comparison to Mastra's RAG mode. |
| **C. `graph`** | `GraphMemory` (grafeo) substring+rank | Weakest config — the honest floor. |
| **D. `hybrid`** | LLM fact extractor → EmbeddingMemory + GraphMemory → RRF fusion | Head-to-head with Mastra's *observational memory* and Zep's Graphiti. |

## Where the numbers come from

- **Dataset**: [`xiaowu0162/longmemeval`](https://huggingface.co/datasets/xiaowu0162/longmemeval), unchanged. Three variants ship: `longmemeval_s` (~115 k tokens / Q, the headline variant), `longmemeval_m` (~1.5 M tokens / Q) and `longmemeval_oracle` (evidence-only control).
- **Judge**: the model given with `--judge-model`, at temperature 0, with the six question-type rubrics **ported verbatim** from Mastra's TypeScript harness (which in turn copies from the official Python evaluator). See `src/judge.rs`; no prompt is modified.
- **Answerer and extractor**: the models given with `--answerer-model` and `--extractor-model`.
- **Metric**: `overall_accuracy` is the **macro average** across question types excluding abstention (matching Mastra's `cli.ts:99`). Abstention is reported as a separate `abstention_accuracy` field.
- **Recorded runs**: the figures published in the [LongMemEval page](../../docs/content/docs/bench-memory.mdx) are one recorded run (2026-04-25) with `gemini-2.5-flash`. That run predates later changes to the code. Re-run before you quote them.

## Credits

- Benchmark & rubric prompts: [Di Wu et al., ICLR 2025](https://arxiv.org/abs/2410.10813) · [official repo](https://github.com/xiaowu0162/LongMemEval).
- Harness shape, question-type handling, abstention detection (`question_id.ends_with("_abs")`): adapted from Mastra's [`@mastra/longmemeval`](https://github.com/mastra-ai/mastra/tree/main/explorations/longmemeval).

## What this benchmark is **not**

- It's not a general "does Bricks beat Mastra at everything" test. It measures long-term conversational recall under a specific rubric.
- It's not apples-to-apples against systems that let the agent call tools during answering (LongMemEval is a single-shot recall benchmark — the retrieval is fixed before the answerer LLM runs).
- It's not cheap to run at scale: a full pass on `longmemeval_s` costs the model calls it makes, which depend on the models you choose.
