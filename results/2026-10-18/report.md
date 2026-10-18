# toolbench report

**Verdict: build aivyx-engine.**

- llama-grammar valid-call rate 71.7 % is below 98.0 %
- llama-grammar correct-choice rate 52.7 % is below 90.0 %
- mistralrs-grammar beats llama-grammar on valid call by 15.3 points (87.0 % vs 71.7 %) at comparable median speed; correct choice 37.0 % vs 52.7 %

| endpoint | model | mode | replies | errors | valid call | correct choice | args | hallucinated | median ms | p95 ms | tok/s |
|---|---|---|---|---|---|---|---|---|---|---|---|
| llama-grammar-4b | /models/Qwen3-4B-Q4_K_M.gguf | Grammar | 300 | 0 | 83.3 % | 61.3 % | 100.0 % | 0 | 1353 | 3985 | 207 |
| llama-grammar | /models/Qwen3.5-9B-Q4_K_M.gguf | Grammar | 300 | 0 | 71.7 % | 52.7 % | 100.0 % | 0 | 679 | 4651 | 111 |
| llama-native-4b | /models/Qwen3-4B-Q4_K_M.gguf | Native | 300 | 0 | 94.9 % | 91.0 % | 100.0 % | 0 | 1559 | 4991 | 160 |
| llama-native | /models/Qwen3.5-9B-Q4_K_M.gguf | Native | 300 | 0 | 97.6 % | 87.7 % | 100.0 % | 0 | 662 | 2147 | 111 |
| mistralrs-grammar-think | default | Grammar | 300 | 0 | 0.0 % | 0.0 % | — | 0 | 375 | 8643 | 117 |
| mistralrs-grammar | default | Grammar | 300 | 0 | 87.0 % | 37.0 % | 98.3 % | 0 | 209 | 8635 | 116 |

## Correct choice by category

- **llama-grammar-4b / /models/Qwen3-4B-Q4_K_M.gguf**: Simple 26/40, SimilarTools 10/40, RequiredArgs 17/40, OptionalArgs 21/30, NoTool 40/40, MultiStep 22/40, RareTool 12/30, Misleading 36/40
- **llama-grammar / /models/Qwen3.5-9B-Q4_K_M.gguf**: Simple 15/40, SimilarTools 5/40, RequiredArgs 14/40, OptionalArgs 16/30, NoTool 38/40, MultiStep 26/40, RareTool 12/30, Misleading 32/40
- **llama-native-4b / /models/Qwen3-4B-Q4_K_M.gguf**: Simple 37/40, SimilarTools 40/40, RequiredArgs 39/40, OptionalArgs 23/30, NoTool 40/40, MultiStep 35/40, RareTool 25/30, Misleading 34/40
- **llama-native / /models/Qwen3.5-9B-Q4_K_M.gguf**: Simple 34/40, SimilarTools 34/40, RequiredArgs 37/40, OptionalArgs 29/30, NoTool 38/40, MultiStep 34/40, RareTool 30/30, Misleading 27/40
- **mistralrs-grammar-think / default**: Simple 0/40, SimilarTools 0/40, RequiredArgs 0/40, OptionalArgs 0/30, NoTool 0/40, MultiStep 0/40, RareTool 0/30, Misleading 0/40
- **mistralrs-grammar / default**: Simple 17/40, SimilarTools 9/40, RequiredArgs 6/40, OptionalArgs 18/30, NoTool 25/40, MultiStep 16/40, RareTool 5/30, Misleading 15/40

## Flaky scenarios

- **llama-grammar-4b / /models/Qwen3-4B-Q4_K_M.gguf**: coder-misleading-deploy, coder-multi-status-then-diff, coder-optional-web, coder-rare-references, coder-similar-move, coder-simple-grep, pa-multi-status-then-diff, pa-optional-git-status, pa-optional-memory-search, pa-rare-dns, pa-rare-role-switch, pa-required-dns, pa-similar-extract, pa-similar-git-diff, pa-simple-fetch
- **llama-grammar / /models/Qwen3.5-9B-Q4_K_M.gguf**: coder-misleading-deploy, coder-misleading-search, coder-multi-read-then-edit, coder-notool-plan, coder-optional-glob, coder-optional-read, coder-rare-references, coder-required-grep, coder-required-write, coder-similar-edit, coder-similar-write, coder-simple-glob, coder-simple-grep, coder-simple-read, coder-simple-shell, pa-misleading-browser, pa-misleading-email, pa-misleading-read-file, pa-multi-read-then-write, pa-multi-search-then-read, pa-notool-explain, pa-optional-git-status, pa-optional-memory-search, pa-optional-read, pa-rare-dns, pa-rare-role-switch, pa-rare-workspace-list, pa-required-dns, pa-required-write, pa-similar-git-diff, pa-simple-fetch, pa-simple-read
- **llama-native-4b / /models/Qwen3-4B-Q4_K_M.gguf**: coder-misleading-deploy, coder-multi-read-then-edit, coder-optional-web, pa-multi-search-then-read, pa-multi-status-then-diff, pa-optional-read, pa-rare-role-switch, pa-required-write, pa-simple-shell
- **llama-native / /models/Qwen3.5-9B-Q4_K_M.gguf**: coder-multi-test-then-answer, coder-notool-plan, coder-similar-edit, pa-misleading-fax, pa-multi-search-then-read, pa-notool-rewrite, pa-optional-read, pa-required-write, pa-similar-git-diff, pa-simple-read
- **mistralrs-grammar / default**: coder-misleading-browser, coder-misleading-open, coder-misleading-search, coder-multi-grep-then-read, coder-multi-status-then-diff, coder-notool-explain, coder-notool-plan, coder-notool-review, coder-notool-thanks, coder-optional-web, coder-required-delete, coder-required-grep, coder-similar-edit, coder-similar-write, coder-simple-glob, coder-simple-grep, coder-simple-read, coder-simple-shell, pa-misleading-browser, pa-misleading-email, pa-misleading-fax, pa-misleading-read-file, pa-multi-read-then-write, pa-multi-search-then-read, pa-notool-math, pa-notool-rewrite, pa-optional-git-status, pa-optional-memory-search, pa-optional-read, pa-rare-dns, pa-rare-role-switch, pa-required-dns, pa-required-memory-write, pa-similar-extract, pa-simple-memory-search, pa-simple-read, pa-simple-shell
