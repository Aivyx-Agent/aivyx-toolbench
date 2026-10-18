# Decision: don't build aivyx-engine for tool calls (2026-10-18)

`report.md` applies the threshold agreed before measuring, and that rule says
**build**. We decided **not** to build, because the data contradicts the rule's
own premise. The rule treated "llama-server + aivyx-stencil grammar" (pa's
Chapter Emboss) as the cheap fix, and asked whether an engine could beat it. The
grammar turned out to be worse than doing nothing.

| setup (Qwen3.5-9B Q4_K_M unless noted) | valid call | correct choice |
|---|---|---|
| llama-server, native `tools` | 97.6 % | 87.7 % |
| llama-server + stencil grammar | 71.7 % | 52.7 % |
| mistral.rs + stencil grammar (thinking off) | 87.0 % | 37.0 % |
| Qwen3-4B, llama-server native | 94.9 % | 91.0 % |
| Qwen3-4B, llama-server + grammar | 83.3 % | 61.3 % |

- **The grammar lowers correct choice everywhere**, most on pa: 36 % with the
  grammar against 83 % native on the 9B. Constrained models mostly pick the
  `respond` text escape where a tool is needed.
- **mistral.rs makes more replies parseable but chooses worse** (37 %). An
  engine built around grammar-constrained decoding, aivyx-engine's core idea,
  would inherit the problem.
- **Native tool calling on llama-server is already close to the bars**: 97.6 % /
  87.7 % on the 9B, while the 4B clears 90 % correct choice.

Caveats:
- **Thinking:** mistral.rs ran with thinking off, because with thinking on the
  constrained JSON landed in `reasoning_content` (the `mistralrs-grammar-think`
  row, 0 %). llama-server ran with thinking on.
- **Ollama** was not measured.
- **Coder scenarios:** seven rows were swapped to tools that exist in each
  product's default configuration (see the scenarios commit).

Found on the way:
- `aivyx-stencil` produced unresolvable grammars for tool schemas with local
  `$defs`. This is fixed in `b848525`, and pa picks it up.
- The spec's "otherwise" follow-up, turning Stencil/Emboss on by default in pa,
  would have made pa worse. It stays **off**.

Follow-ups:
- Keep `constrain_tool_calls` off by default in pa.
- Find out why constrained models prefer `respond`: the preamble wording, a lazy
  grammar that only applies once a call starts, or no text escape at all.
- Re-measure native tool calling when models or servers change (`toolbench run`).

The engine's other goals (zero-setup local models, shared scheduling, process
isolation) were not measured here and remain open.
