# Decision: don't build aivyx-engine for tool calls (2026-10-18)

`report.md` applies the threshold agreed before measuring, and that rule says
**build**. We decided **not** to build. The rule treated "llama-server +
aivyx-stencil grammar" (pa's Chapter Emboss) as the cheap fix, and asked whether
an engine could beat it. On this data, that grammar chose tools worse than
llama-server's native tool calling, so the rule's baseline is not a good one.

| setup (Qwen3.5-9B Q4_K_M unless noted) | valid call | correct choice |
|---|---|---|
| llama-server, native `tools` | 97.6 % | 87.7 % |
| llama-server + stencil grammar | 71.7 % | 52.7 % |
| mistral.rs + stencil grammar (thinking off) | 87.0 % | 37.0 % |
| mistral.rs + stencil grammar (thinking on) | 0 % | 0 % |
| Qwen3-4B, llama-server native | 94.9 % | 91.0 % |
| Qwen3-4B, llama-server + grammar | 83.3 % | 61.3 % |

- **This grammar (with `arguments` before `name`) lowered tool choice** in every
  setup measured, most on pa: 36.0 % correct choice with the grammar against
  83.3 % native on the 9B.
- **Misses on scenarios that expect a call are mostly a wrong real tool, not the
  `respond` text escape.** Wrong tool / `respond` / unparseable: llama-grammar
  9B 71 / 63 / 3; llama-grammar 4B 72 / 33 / 7; mistralrs-grammar 136 / 15 / 11.
  Only pa on the 9B has `respond` close to wrong tool (47 vs 43).
- **mistral.rs did not make more replies parseable.** It had 19 unparseable
  replies (all `finish_reason=length` at `max_tokens` 1024, many of them runaway
  whitespace that the grammar allows) against 5 for llama-grammar. Its higher
  valid-call rate comes from calling some tool where llama-grammar answered
  `respond`; a text reply when a call was expected counts as an invalid call.
  Its correct choice is lower (37.0 % vs 52.7 %).
- **Likely cause, key order.** `aivyx-stencil` builds the grammar with
  `serde_json` without `preserve_order`, so object keys come out alphabetical:
  `arguments` before `name` in every branch (pa's frozen golden test shows the
  same). Both servers enforce that order: 295 of 300 llama-grammar replies and
  281 of 300 mistral.rs replies are `{"arguments":…,"name":…}`, although
  `RESPOND_PREAMBLE` asks for name first. The model writes the arguments before
  it commits to a tool, and the name is then forced to fit them (for example 41
  replies named `git_push` with other tools' arguments). This looks like a
  fixable defect in `aivyx-stencil`, not something inherent to constrained
  decoding. We have not tested a name-first grammar.
- **Native tool calling on llama-server is already close to the bars**: 97.6 % /
  87.7 % on the 9B, while the 4B reaches 91.0 % correct choice.
- **Consequence for the engine.** An engine built on constrained decoding is not
  justified by this data. Any gain from it would have to come from a fixed
  grammar, which is cheap to try first on llama-server.

Caveats:
- **Key order:** see above. The grammar measured here is the one with
  `arguments` first.
- **Thinking:** llama-server ran with thinking on (290 of 300 grammar replies
  and all native replies carry `reasoning_content`, so llama.cpp let the model
  reason before the grammar applied). mistral.rs ran with `enable_thinking =
  false`, because with thinking on its JSON landed in `reasoning_content` (the
  `mistralrs-grammar-think` row, 0 %). The two grammar rows are therefore not
  like for like.
- **`max_tokens` 1024 for all endpoints, and some replies were cut off.**
  Truncations: llama-native 9B 0, llama-grammar 5, llama-grammar-4b 7,
  llama-native-4b 7 (about 2.3 points of its 94.9 % valid call),
  mistralrs-grammar 19, mistralrs-grammar-think 23.
- **Per-endpoint settings are not in the result files.** They live in
  `toolbench.toml` (see the commits `30aaecb` and `4c69f04`): llama-grammar used
  the `json_schema` field; mistral.rs used `response_format` plus `extra_body`
  `enable_thinking = false`. The native runs come from the first live session;
  the grammar runs were re-run after the `$defs` fix.
- **Condition 2 fired on valid call alone** (87.0 % vs 71.7 %). Correct choice
  was 15.7 points worse. "Comparable speed" compared medians (209 vs 679 ms);
  the mistral.rs p95 was worse (8635 vs 4651 ms).
- **Latency is informational only.** The GPU was shared between runs.
- **Scenario swaps:** seven rows were swapped to tools that exist in each
  product's default configuration (3 pa rows from `git.*` to `shell.exec`, 4
  coder rows) in `5bf447a`. `5f8df58` then re-targeted two coder rows and
  tightened the argument checks on four pa rows.
- **Ollama** was not measured.

Found on the way:
- `aivyx-stencil` produced unresolvable grammars for tool schemas with local
  `$defs`. This is fixed in `b848525`, and pa picks it up.
- The spec's "otherwise" follow-up, turning Stencil/Emboss on by default in pa,
  would have made pa worse on this data. It stays **off**.

Follow-ups, in order:
1. Make the grammar emit `name` first, then re-measure llama-grammar and
   mistralrs-grammar with matching thinking settings. Caution: enabling
   `serde_json/preserve_order` in `aivyx-stencil` would switch it on across all
   of pa through feature unification and change the golden test. Alternatives
   are building the grammar text directly, or using llama-server's GBNF
   `grammar` field.
2. Keep `constrain_tool_calls` off in pa until then.
3. Re-measure native tool calling when models or servers change
   (`toolbench run`).

The engine's other goals (zero-setup local models, shared scheduling, process
isolation) were not measured here and remain open.
