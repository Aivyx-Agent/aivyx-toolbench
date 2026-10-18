# toolbench report

**Verdict: build aivyx-engine.**

- llama-grammar valid-call rate 82.4 % [72.6–91.2] is below 98.0 %, clearly
- llama-grammar correct-choice rate 76.7 % [67.0–86.3] is below 90.0 %, clearly
- baseline and challenger temperatures differ or are unknown (? vs ?)

| endpoint | model | run | date | temp | mode | replies | errors | extra calls | valid call | correct choice | args | hallucinated | median ms | p95 ms | tok/s |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| llama-grammar | /models/Qwen3.5-9B-Q4_K_M.gguf | legacy:llama-grammar | — | ? | Grammar | 300 | 0 | 0 | 82.4 % [72.6–91.2] | 76.7 % [67.0–86.3] | 100.0 % [100.0–100.0] | 0 | 624 | 4138 | 116 |
| llama-grammar-nothink | /models/Qwen3.5-9B-Q4_K_M.gguf | legacy:llama-grammar-nothink | — | ? | Grammar | 300 | 0 | 0 | 80.4 % [69.8–90.2] | 74.7 % [64.7–84.3] | 100.0 % [100.0–100.0] | 0 | 249 | 929 | 106 |
| llama-native | /models/Qwen3.5-9B-Q4_K_M.gguf | legacy:llama-native | — | ? | Native | 300 | 0 | 4 | 97.6 % [93.1–100.0] | 87.7 % [80.7–94.0] | 100.0 % [100.0–100.0] | 0 | 662 | 2147 | 111 |
| mistralrs-grammar | default | legacy:mistralrs-grammar | — | ? | Grammar | 300 | 0 | 0 | 84.0 % [75.2–92.1] | 72.0 % [62.0–81.7] | 100.0 % [100.0–100.0] | 0 | 210 | 1070 | 115 |

## Correct choice by category

- **llama-grammar / /models/Qwen3.5-9B-Q4_K_M.gguf (legacy:llama-grammar)**: Simple 33/40, SimilarTools 22/40, RequiredArgs 26/40, OptionalArgs 21/30, NoTool 40/40, MultiStep 34/40, RareTool 21/30, Misleading 33/40
- **llama-grammar-nothink / /models/Qwen3.5-9B-Q4_K_M.gguf (legacy:llama-grammar-nothink)**: Simple 30/40, SimilarTools 21/40, RequiredArgs 29/40, OptionalArgs 18/30, NoTool 40/40, MultiStep 35/40, RareTool 19/30, Misleading 32/40
- **llama-native / /models/Qwen3.5-9B-Q4_K_M.gguf (legacy:llama-native)**: Simple 34/40, SimilarTools 34/40, RequiredArgs 37/40, OptionalArgs 29/30, NoTool 38/40, MultiStep 34/40, RareTool 30/30, Misleading 27/40
- **mistralrs-grammar / default (legacy:mistralrs-grammar)**: Simple 30/40, SimilarTools 22/40, RequiredArgs 29/40, OptionalArgs 20/30, NoTool 38/40, MultiStep 34/40, RareTool 16/30, Misleading 27/40

## Flaky scenarios

- **llama-grammar / /models/Qwen3.5-9B-Q4_K_M.gguf (legacy:llama-grammar)**: coder-misleading-deploy, coder-multi-test-then-answer, pa-optional-read, pa-rare-dns, pa-required-dns, pa-required-write, pa-similar-extract, pa-similar-git-diff, pa-similar-metadata, pa-simple-read
- **llama-grammar-nothink / /models/Qwen3.5-9B-Q4_K_M.gguf (legacy:llama-grammar-nothink)**: coder-misleading-open, coder-rare-references, coder-required-delete, coder-similar-edit, coder-similar-write, coder-simple-grep, pa-optional-memory-search, pa-optional-read, pa-similar-git-diff, pa-simple-fetch, pa-simple-read
- **llama-native / /models/Qwen3.5-9B-Q4_K_M.gguf (legacy:llama-native)**: coder-multi-test-then-answer, coder-notool-plan, coder-similar-edit, pa-misleading-fax, pa-multi-search-then-read, pa-notool-rewrite, pa-optional-read, pa-required-write, pa-similar-git-diff, pa-simple-read
- **mistralrs-grammar / default (legacy:mistralrs-grammar)**: coder-misleading-deploy, coder-misleading-open, coder-misleading-search, coder-notool-review, coder-rare-references, coder-required-branch, coder-simple-grep, pa-misleading-browser, pa-misleading-read-file, pa-multi-search-then-read, pa-notool-rewrite, pa-optional-memory-search, pa-optional-read, pa-required-dns, pa-required-memory-write, pa-required-post, pa-similar-extract, pa-simple-fetch, pa-simple-read
