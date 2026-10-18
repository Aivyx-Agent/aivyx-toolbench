# toolbench report

**Verdict: build aivyx-engine.**

- llama-grammar valid-call rate 60.2 % is below 98.0 %
- llama-grammar correct-choice rate 19.0 % is below 90.0 %

| endpoint | model | mode | replies | errors | valid call | correct choice | args | hallucinated | median ms | p95 ms | tok/s |
|---|---|---|---|---|---|---|---|---|---|---|---|
| llama-grammar-4b | /models/Qwen3-4B-Q4_K_M.gguf | Grammar | grammar unsupported | | | | | | | | |
| llama-grammar | /models/Qwen3.5-9B-Q4_K_M.gguf | Grammar | 300 | 150 | 60.2 % | 19.0 % | 100.0 % | 0 | 1053 | 5661 | 111 |
| llama-native-4b | /models/Qwen3-4B-Q4_K_M.gguf | Native | 300 | 0 | 94.9 % | 91.0 % | 100.0 % | 0 | 1559 | 4991 | 160 |
| llama-native | /models/Qwen3.5-9B-Q4_K_M.gguf | Native | 300 | 0 | 97.6 % | 87.7 % | 100.0 % | 0 | 662 | 2147 | 111 |

## Correct choice by category

- **llama-grammar / /models/Qwen3.5-9B-Q4_K_M.gguf**: Simple 5/40, SimilarTools 1/40, RequiredArgs 2/40, OptionalArgs 6/30, NoTool 18/40, MultiStep 5/40, RareTool 4/30, Misleading 16/40
- **llama-native-4b / /models/Qwen3-4B-Q4_K_M.gguf**: Simple 37/40, SimilarTools 40/40, RequiredArgs 39/40, OptionalArgs 23/30, NoTool 40/40, MultiStep 35/40, RareTool 25/30, Misleading 34/40
- **llama-native / /models/Qwen3.5-9B-Q4_K_M.gguf**: Simple 34/40, SimilarTools 34/40, RequiredArgs 37/40, OptionalArgs 29/30, NoTool 38/40, MultiStep 34/40, RareTool 30/30, Misleading 27/40

## Flaky scenarios

- **llama-grammar / /models/Qwen3.5-9B-Q4_K_M.gguf**: pa-misleading-browser, pa-misleading-email, pa-misleading-read-file, pa-multi-read-then-write, pa-multi-status-then-diff, pa-notool-explain, pa-notool-greeting, pa-optional-git-status, pa-optional-memory-search, pa-optional-read, pa-rare-dns, pa-rare-workspace-list, pa-required-dns, pa-required-memory-write, pa-required-post, pa-required-write, pa-similar-git-diff, pa-similar-metadata, pa-similar-workspace-note, pa-simple-fetch, pa-simple-memory-search
- **llama-native-4b / /models/Qwen3-4B-Q4_K_M.gguf**: coder-misleading-deploy, coder-multi-read-then-edit, coder-optional-web, pa-multi-search-then-read, pa-multi-status-then-diff, pa-optional-read, pa-rare-role-switch, pa-required-write, pa-simple-shell
- **llama-native / /models/Qwen3.5-9B-Q4_K_M.gguf**: coder-multi-test-then-answer, coder-notool-plan, coder-similar-edit, pa-misleading-fax, pa-multi-search-then-read, pa-notool-rewrite, pa-optional-read, pa-required-write, pa-similar-git-diff, pa-simple-read
