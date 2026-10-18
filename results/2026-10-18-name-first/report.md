# toolbench report

**Verdict: build aivyx-engine.**

- llama-grammar valid-call rate 82.4 % is below 98.0 %
- llama-grammar correct-choice rate 76.7 % is below 90.0 %

| endpoint | model | mode | replies | errors | valid call | correct choice | args | hallucinated | median ms | p95 ms | tok/s |
|---|---|---|---|---|---|---|---|---|---|---|---|
| llama-grammar-nothink | /models/Qwen3.5-9B-Q4_K_M.gguf | Grammar | 300 | 0 | 80.4 % | 74.7 % | 100.0 % | 0 | 249 | 929 | 106 |
| llama-grammar | /models/Qwen3.5-9B-Q4_K_M.gguf | Grammar | 300 | 0 | 82.4 % | 76.7 % | 100.0 % | 0 | 624 | 4138 | 116 |
| llama-native | /models/Qwen3.5-9B-Q4_K_M.gguf | Native | 300 | 0 | 97.6 % | 87.7 % | 100.0 % | 0 | 662 | 2147 | 111 |
| mistralrs-grammar | default | Grammar | 300 | 0 | 84.0 % | 72.0 % | 100.0 % | 0 | 210 | 1070 | 115 |

## Correct choice by category

- **llama-grammar-nothink / /models/Qwen3.5-9B-Q4_K_M.gguf**: Simple 30/40, SimilarTools 21/40, RequiredArgs 29/40, OptionalArgs 18/30, NoTool 40/40, MultiStep 35/40, RareTool 19/30, Misleading 32/40
- **llama-grammar / /models/Qwen3.5-9B-Q4_K_M.gguf**: Simple 33/40, SimilarTools 22/40, RequiredArgs 26/40, OptionalArgs 21/30, NoTool 40/40, MultiStep 34/40, RareTool 21/30, Misleading 33/40
- **llama-native / /models/Qwen3.5-9B-Q4_K_M.gguf**: Simple 34/40, SimilarTools 34/40, RequiredArgs 37/40, OptionalArgs 29/30, NoTool 38/40, MultiStep 34/40, RareTool 30/30, Misleading 27/40
- **mistralrs-grammar / default**: Simple 30/40, SimilarTools 22/40, RequiredArgs 29/40, OptionalArgs 20/30, NoTool 38/40, MultiStep 34/40, RareTool 16/30, Misleading 27/40

## Flaky scenarios

- **llama-grammar-nothink / /models/Qwen3.5-9B-Q4_K_M.gguf**: coder-misleading-open, coder-rare-references, coder-required-delete, coder-similar-edit, coder-similar-write, coder-simple-grep, pa-optional-memory-search, pa-optional-read, pa-similar-git-diff, pa-simple-fetch, pa-simple-read
- **llama-grammar / /models/Qwen3.5-9B-Q4_K_M.gguf**: coder-misleading-deploy, coder-multi-test-then-answer, pa-optional-read, pa-rare-dns, pa-required-dns, pa-required-write, pa-similar-extract, pa-similar-git-diff, pa-similar-metadata, pa-simple-read
- **llama-native / /models/Qwen3.5-9B-Q4_K_M.gguf**: coder-multi-test-then-answer, coder-notool-plan, coder-similar-edit, pa-misleading-fax, pa-multi-search-then-read, pa-notool-rewrite, pa-optional-read, pa-required-write, pa-similar-git-diff, pa-simple-read
- **mistralrs-grammar / default**: coder-misleading-deploy, coder-misleading-open, coder-misleading-search, coder-notool-review, coder-rare-references, coder-required-branch, coder-simple-grep, pa-misleading-browser, pa-misleading-read-file, pa-multi-search-then-read, pa-notool-rewrite, pa-optional-memory-search, pa-optional-read, pa-required-dns, pa-required-memory-write, pa-required-post, pa-similar-extract, pa-simple-fetch, pa-simple-read
