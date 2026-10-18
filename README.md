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
- `toolbench capture --product <p> --source <s> --out <file>` runs a fake server that saves a product's first request with tools as a snapshot (see Capture below).
- `toolbench run --endpoint <name> --out <file.jsonl>` sends every scenario to one endpoint from `toolbench.toml` (`--repeat` default 5, `--timeout` default 120 s).
- `toolbench rescore <in.jsonl> --out <out.jsonl>` re-applies the checks to stored results.
- `toolbench report <files.jsonl...> --out <report.md>` writes the comparison and verdict (`--out` creates its parent directory).

Example:

```sh
cargo run --release -p toolbench -- run --endpoint llama-native --out results/run/llama-native.jsonl
cargo run --release -p toolbench -- report results/run/*.jsonl --config toolbench.toml --out results/run/report.md
```

### Run files

`run` writes a `{"type":"run",...}` header as the first line: the `run_id`
(`YYYYMMDDTHHMMSSZ-xxxx`), the settings, the toolbench commit, the sha256 of the
snapshots and scenarios, and the models the server reports. Every reply and
status line carries that `run_id`, and a closing `run_end` line marks the run as
complete; a header with no `run_end` is shown as incomplete. Files written
before headers existed have none and are read as `legacy:<file stem>` runs. When two
inputs share a stem, later ones are named `legacy:<parent dir>/<stem>`, so the
id to pass to `--run` depends on which files you give, and in what order.
Run headers include the endpoint's `base_url` and the server's model list:
check them (privacy grep) before committing results.

### Report

`report` shows one row per run with 95 % confidence intervals (a cluster
bootstrap over scenarios: 2000 resamples, fixed seed). The verdict says
*clearly* when the intervals support it and *within noise* when they do not.
By default the verdict uses the runs of the endpoints named in `[verdict]`;
choose them explicitly with `--run NAME=RUN_ID` (repeatable, one per endpoint)
or take each endpoint's newest run with `--latest`.

### Capture

`capture` redacts the home path, user name, emails, non-loopback IPv4 addresses
and URL credentials (in the prompt, tool descriptions and parameter schemas),
and flags token-like or personal-looking lines. It prints a review to stderr and
saves only with `--accept` or a `y` at a terminal; otherwise it prints
"not saved: re-run with --accept after reviewing". `--no-redact` skips
redaction. A non-loopback `--listen` address needs `--allow-remote`.

### toolbench.toml

Each `[[endpoint]]` has:

- `name`, `base_url`, `model`: the endpoint name, its OpenAI-compatible base URL and the model id to send.
- `mode`: `native` (tools in the request) or `grammar` (the stencil grammar as a constraint).
- `grammar_field`: `json_schema` or `response_format`; where the grammar goes in the request (grammar mode).
- `max_tokens`: default 1024.
- `temperature`: sampling temperature, sent only when set. A `[defaults] temperature` fills endpoints that have none. `extra_body` wins over both, and the run header records the effective value.
- `extra_body`: a table merged last into the request body, so it can override any other field. Use it only for server switches such as `enable_thinking`.

A `[verdict]` section names the baseline and challenger endpoints.

## Results

- `results/2026-10-18/`: the first run. `report.md` has the numbers and
  `DECISION.md` the decision and its caveats.
- `results/2026-10-18-name-first/`: the follow-up after the grammar's `name`
  key was moved first, with its own `report.md`.

These files predate run headers and are read as `legacy:` runs.

Source-available under BUSL-1.1.
