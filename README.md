# aivyx-toolbench

[![CI](https://github.com/Aivyx-Agent/aivyx-toolbench/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/Aivyx-Agent/aivyx-toolbench/actions/workflows/ci.yml)
[![License: BUSL-1.1](https://img.shields.io/badge/license-BUSL--1.1-blue.svg)](LICENSE)

Measures how reliably a local model server produces valid tool calls for
Aivyx PA's and aivyx-coder's real tool lists.

- **`aivyx-stencil`** — the tool-call grammar: a JSON Schema that admits only
  a well-formed call to a real tool (or a `respond` text escape). Extracted
  from Aivyx PA (Chapters Stencil and Emboss); pa pins it by rev.
- **`toolbench`** — sends synthetic scenarios to any OpenAI-compatible
  endpoint, scores every reply (parseable → real tool → schema-valid →
  right choice → argument checks), and writes a comparison report.

## Usage

Run from the repository root. Commands:

- `toolbench tools <snapshot>` lists a snapshot's tools with required and optional arguments.
- `toolbench capture --product <p> --source <s> --out <file>` runs a fake server that saves a product's first request with tools as a snapshot.
- `toolbench run --endpoint <name> --out <file.jsonl>` sends every scenario to one endpoint from `toolbench.toml` (`--repeat` default 5, `--timeout` default 120 s).
- `toolbench rescore <in.jsonl> --out <out.jsonl>` re-applies the checks to stored results.
- `toolbench report <files.jsonl...> --out <report.md>` writes the comparison and verdict.

Example:

```sh
cargo run --release -p toolbench -- run --endpoint llama-native --out results/run/llama-native.jsonl
cargo run --release -p toolbench -- report results/run/*.jsonl --config toolbench.toml --out results/run/report.md
```

### toolbench.toml

Each `[[endpoint]]` has:

- `name`, `base_url`, `model`: the endpoint name, its OpenAI-compatible base URL and the model id to send.
- `mode`: `native` (tools in the request) or `grammar` (the stencil grammar as a constraint).
- `grammar_field`: `json_schema` or `response_format`; where the grammar goes in the request (grammar mode).
- `max_tokens`: default 1024.
- `extra_body`: a table merged last into the request body, so it can override any other field. Use it only for server switches such as `enable_thinking`.

A `[verdict]` section names the baseline and challenger endpoints.

## Results

The 2026-10-18 run is in `results/2026-10-18/`: `report.md` has the numbers and
`DECISION.md` the decision and its caveats.

Source-available under BUSL-1.1.
