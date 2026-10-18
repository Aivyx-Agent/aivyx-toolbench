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

Source-available under BUSL-1.1.
