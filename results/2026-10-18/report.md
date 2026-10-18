# toolbench report

**Verdict: build aivyx-engine.**

- llama-grammar valid-call rate 71.7 % [61.4–81.4] is below 98.0 %, clearly
- llama-grammar correct-choice rate 52.7 % [42.3–63.3] is below 90.0 %, clearly
- mistralrs-grammar beats llama-grammar on valid call by +15.3 pts overall; paired over 50 shared scenarios +17.5 pts [8.6–27.7] (clearly), at comparable median speed; correct choice 37.0 % vs 52.7 %
- baseline and challenger temperatures differ or are unknown (? vs ?)

| endpoint | model | run | date | temp | mode | replies | errors | extra calls | valid call | correct choice | args | hallucinated | median ms | p95 ms | tok/s |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| llama-grammar | /models/Qwen3.5-9B-Q4_K_M.gguf | legacy:llama-grammar | — | ? | Grammar | 300 | 0 | 0 | 71.7 % [61.4–81.4] | 52.7 % [42.3–63.3] | 100.0 % [100.0–100.0] | 0 | 679 | 4651 | 111 |
| llama-grammar-4b | /models/Qwen3-4B-Q4_K_M.gguf | legacy:llama-grammar-4b | — | ? | Grammar | 300 | 0 | 0 | 83.3 % [73.6–91.2] | 61.3 % [50.7–72.3] | 100.0 % [100.0–100.0] | 0 | 1353 | 3985 | 207 |
| llama-native | /models/Qwen3.5-9B-Q4_K_M.gguf | legacy:llama-native | — | ? | Native | 300 | 0 | 4 | 97.6 % [93.1–100.0] | 87.7 % [80.7–94.0] | 100.0 % [100.0–100.0] | 0 | 662 | 2147 | 111 |
| llama-native-4b | /models/Qwen3-4B-Q4_K_M.gguf | legacy:llama-native-4b | — | ? | Native | 300 | 0 | 1 | 94.9 % [90.4–98.7] | 91.0 % [84.3–96.7] | 100.0 % [100.0–100.0] | 0 | 1559 | 4991 | 160 |
| mistralrs-grammar | default | legacy:mistralrs-grammar | — | ? | Grammar | 300 | 0 | 0 | 87.0 % [80.6–92.4] | 37.0 % [28.0–47.0] | 98.3 % [93.8–100.0] | 0 | 209 | 8635 | 116 |
| mistralrs-grammar-think | default | legacy:mistralrs-grammar-think | — | ? | Grammar | 300 | 0 | 0 | 0.0 % [0.0–0.0] | 0.0 % [0.0–0.0] | — | 0 | 375 | 8643 | 117 |

## Correct choice by category

- **llama-grammar / /models/Qwen3.5-9B-Q4_K_M.gguf (legacy:llama-grammar)**: Simple 15/40, SimilarTools 5/40, RequiredArgs 14/40, OptionalArgs 16/30, NoTool 38/40, MultiStep 26/40, RareTool 12/30, Misleading 32/40
- **llama-grammar-4b / /models/Qwen3-4B-Q4_K_M.gguf (legacy:llama-grammar-4b)**: Simple 26/40, SimilarTools 10/40, RequiredArgs 17/40, OptionalArgs 21/30, NoTool 40/40, MultiStep 22/40, RareTool 12/30, Misleading 36/40
- **llama-native / /models/Qwen3.5-9B-Q4_K_M.gguf (legacy:llama-native)**: Simple 34/40, SimilarTools 34/40, RequiredArgs 37/40, OptionalArgs 29/30, NoTool 38/40, MultiStep 34/40, RareTool 30/30, Misleading 27/40
- **llama-native-4b / /models/Qwen3-4B-Q4_K_M.gguf (legacy:llama-native-4b)**: Simple 37/40, SimilarTools 40/40, RequiredArgs 39/40, OptionalArgs 23/30, NoTool 40/40, MultiStep 35/40, RareTool 25/30, Misleading 34/40
- **mistralrs-grammar / default (legacy:mistralrs-grammar)**: Simple 17/40, SimilarTools 9/40, RequiredArgs 6/40, OptionalArgs 18/30, NoTool 25/40, MultiStep 16/40, RareTool 5/30, Misleading 15/40
- **mistralrs-grammar-think / default (legacy:mistralrs-grammar-think)**: Simple 0/40, SimilarTools 0/40, RequiredArgs 0/40, OptionalArgs 0/30, NoTool 0/40, MultiStep 0/40, RareTool 0/30, Misleading 0/40

## Flaky scenarios

- **llama-grammar / /models/Qwen3.5-9B-Q4_K_M.gguf (legacy:llama-grammar)**: coder-misleading-deploy, coder-misleading-search, coder-multi-read-then-edit, coder-notool-plan, coder-optional-glob, coder-optional-read, coder-rare-references, coder-required-grep, coder-required-write, coder-similar-edit, coder-similar-write, coder-simple-glob, coder-simple-grep, coder-simple-read, coder-simple-shell, pa-misleading-browser, pa-misleading-email, pa-misleading-read-file, pa-multi-read-then-write, pa-multi-search-then-read, pa-notool-explain, pa-optional-git-status, pa-optional-memory-search, pa-optional-read, pa-rare-dns, pa-rare-role-switch, pa-rare-workspace-list, pa-required-dns, pa-required-write, pa-similar-git-diff, pa-simple-fetch, pa-simple-read
- **llama-grammar-4b / /models/Qwen3-4B-Q4_K_M.gguf (legacy:llama-grammar-4b)**: coder-misleading-deploy, coder-multi-status-then-diff, coder-optional-web, coder-rare-references, coder-similar-move, coder-simple-grep, pa-multi-status-then-diff, pa-optional-git-status, pa-optional-memory-search, pa-rare-dns, pa-rare-role-switch, pa-required-dns, pa-similar-extract, pa-similar-git-diff, pa-simple-fetch
- **llama-native / /models/Qwen3.5-9B-Q4_K_M.gguf (legacy:llama-native)**: coder-multi-test-then-answer, coder-notool-plan, coder-similar-edit, pa-misleading-fax, pa-multi-search-then-read, pa-notool-rewrite, pa-optional-read, pa-required-write, pa-similar-git-diff, pa-simple-read
- **llama-native-4b / /models/Qwen3-4B-Q4_K_M.gguf (legacy:llama-native-4b)**: coder-misleading-deploy, coder-multi-read-then-edit, coder-optional-web, pa-multi-search-then-read, pa-multi-status-then-diff, pa-optional-read, pa-rare-role-switch, pa-required-write, pa-simple-shell
- **mistralrs-grammar / default (legacy:mistralrs-grammar)**: coder-misleading-browser, coder-misleading-open, coder-misleading-search, coder-multi-grep-then-read, coder-multi-status-then-diff, coder-notool-explain, coder-notool-plan, coder-notool-review, coder-notool-thanks, coder-optional-web, coder-required-delete, coder-required-grep, coder-similar-edit, coder-similar-write, coder-simple-glob, coder-simple-grep, coder-simple-read, coder-simple-shell, pa-misleading-browser, pa-misleading-email, pa-misleading-fax, pa-misleading-read-file, pa-multi-read-then-write, pa-multi-search-then-read, pa-notool-math, pa-notool-rewrite, pa-optional-git-status, pa-optional-memory-search, pa-optional-read, pa-rare-dns, pa-rare-role-switch, pa-required-dns, pa-required-memory-write, pa-similar-extract, pa-simple-memory-search, pa-simple-read, pa-simple-shell
