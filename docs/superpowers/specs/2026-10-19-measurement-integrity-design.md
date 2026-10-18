# toolbench measurement integrity — design

Date: 2026-10-19 · Status: approved design · Source: the 2026-10-19 audit
(findings T1–T8).

## Why

toolbench's job is to produce numbers a decision can rest on. The audit found
that it can't yet guarantee that:

- **T1:** `report` silently merges separate runs of one endpoint and model.
  Old and name-first `llama-grammar` became one row at 64.7 %, which neither
  run measured.
- **T2:** results don't record their settings, toolbench commit, snapshot or
  server.
- **T3:** no uncertainty is shown. Correct-choice 95 % CIs are about ±10
  points, and the 5-point edge and the 98/90 bars sit inside that noise.
- **T4:** temperature is the server's default and is never recorded.
- **T5:** capture privacy rests on a manual grep.
- **T6–T8:** Dependabot off, no `cargo deny` in CI, extra tool calls
  unreported, `--listen` and `report --out` gaps, an unescaped Markdown
  table, a stale README.

## 1. Run identity and metadata (T1, T2, T4)

Each `toolbench run` writes, before any reply, one header line:

```json
{"type":"run","run_id":"20261019T081502Z-3f9a","started_at":"2026-10-19T08:15:02Z",
 "toolbench":{"version":"0.1.0","commit":"6f21334"},
 "endpoint":{"name":"…","base_url":"…","model":"…","mode":"grammar","grammar_field":"json_schema",
             "max_tokens":1024,"temperature":null,"extra_body":{}},
 "repeat":5,"timeout_s":120,
 "snapshots":{"pa":{"source":"…","sha256":"…"},"coder":{"source":"…","sha256":"…"}},
 "scenarios":{"count":60,"sha256":"…"},
 "server":{"models":["…"],"fingerprint":null}}
```

- **`run_id`:** UTC timestamp `YYYYMMDDTHHMMSSZ` plus `-` and 4 random hex
  characters.
- **`toolbench.commit`:** from `build.rs` (`git rev-parse --short HEAD`), or
  `"unknown"` outside a git checkout.
- **`snapshots.*.sha256`:** a hash of each fixture file's bytes.
- **`scenarios.sha256`:** a hash of the sorted scenarios' canonical JSON
  serialisation.
- **`server.models`:** the ids from the reachability probe's `GET /models`.
- **`server.fingerprint`:** the first successful reply's `system_fingerprint`
  (`null` if none). It is written to a final `{"type":"run_end", "run_id":…,
  "fingerprint":…, "replies":N}` line, so the header can be written before
  any request.
- **Reply and status lines** carry `run_id`.
- **`rescore`** keeps the header and `run_id`s and adds
  `"rescored_by": {"version", "commit"}` to the header. A legacy input (no
  header) is rescored without adding one, so its lines stay legacy.
- **Legacy files** (no header) are read as one run per file, with
  `run_id = "legacy:<file stem>"`, `temperature` unknown and no metadata.
  Committed results are never rewritten.

**Temperature:** an optional `temperature` per `[[endpoint]]`, plus an
optional `[defaults] temperature` that applies to endpoints without one.
- It is sent only when set. `extra_body` still wins if it sets
  `temperature`; the header records the effective value.
- `null` in the header means "server default".

## 2. Report and verdict (T1 display, T3, T4 warnings, T7, T8)

- **One row per run.** Stats are keyed by `run_id`.
  - Rows show the endpoint, model, short run id (the first 13 characters or
    `legacy:…`), the run date, and the temperature (`default`, a number, or
    `?` for legacy).
  - The category breakdown and flaky lists are per run.
- **CIs:** valid call, correct choice and args get a 95 % CI, printed as
  `76.7 % [66.3–86.0]`.
  - Method: a cluster bootstrap that resamples scenarios with replacement
    (all of a scenario's runs together), 2000 resamples, percentile
    interval.
  - It uses a small deterministic PRNG (splitmix64, seed 0x5eed) implemented
    in the crate, with no new dependency, so the same input gives the same
    CI.
  - A rate with no denominator prints `—`.
- **Verdict (same rule, same constants):**
  - **Bar reasons** add *clearly* when the CI lies entirely on the deciding
    side of the bar, otherwise *within noise*.
  - **Challenger edge:** a paired bootstrap of (challenger − baseline) over
    the scenarios both runs share, reported as `+15.3 pts [6.1–24.0]`. It is
    *clearly* when the CI's lower bound is ≥ 5 points, otherwise *within
    noise*.
  - When any deciding condition is within noise, the verdict headline adds
    "(at least one deciding condition is within noise)". The
    Build / DontBuild / Undecided outcome is computed exactly as today.
  - The verdict looks endpoints up by name. More than one matching run (after
    `--run`/`--latest`) → **Undecided** with "N runs of `<name>`; choose one
    with --run <name>=<run_id> or --latest".
  - When the baseline's and challenger's temperatures differ, or either is
    unknown, the reasons gain one line saying so.
- **Flags:** `report --run <endpoint>=<run_id>` (repeatable) and `--latest`
  (for each endpoint name, the run with the newest `started_at`; legacy runs
  sort by file order) restrict which runs enter the verdict. The table still
  shows every run.
- **Other report changes:**
  - The table gains an "extra calls" column (replies with more than one tool
    call).
  - `|` in cells is escaped.
  - `report --out` creates its parent directory.

## 3. Capture guard (T5)

Before a snapshot is written, capture does three things.

**1. Redaction**, in the system prompt and every tool description:
- `$HOME` and its canonical path → `/home/user`.
- The current user name, as a whole word (from `USER`/`LOGNAME`, or the
  home directory's last component) → `user`.
- Email-like `[\w.+-]+@[\w-]+\.[\w.]+` → `user@example.com`.
- Non-loopback IPv4 → `192.0.2.1`.
- URL userinfo (`scheme://user:pass@`) → `scheme://`.

The counts are appended to `source` (`; redacted: home path ×6, user name ×1`).
`--no-redact` skips this step, with a warning.

**2. Flags**, for anything it can't decide:
- Lines with a 32+ character run of `[A-Za-z0-9+/=_-]` (token-like).
- Headings or lines matching (case-insensitive) `memory`, `memories`,
  `persona`, `about the user`, `learned`, `preferences`.

**3. Review**, printed to stderr:
- the product, tool count and prompt length;
- the redaction counts;
- each flagged line, with its number and 80 characters of context.

The file is written only if `--accept` was given, or stdin is a terminal and
the user answers `y`. Otherwise capture prints "not saved: re-run with
--accept after reviewing" and writes nothing. The server keeps answering, and
capture waits for the next tools request after a decline.

**`--listen`:** a non-loopback address requires `--allow-remote`, and a
warning is printed.

## 4. Hygiene, docs and results (T6, T8)

- **Dependabot:** enable vulnerability alerts on `Aivyx-Agent/aivyx-toolbench`
  (automated security fixes off).
- **CI:** a `cargo deny check` job via `EmbarkStudios/cargo-deny-action` using
  the existing `deny.toml`. Fix what it flags, or record an exception with
  its reason in `deny.toml`.
- **README:** Usage covers the run header, `--run`/`--latest`, `temperature`
  and `[defaults]`, and the capture review and `--accept`. Results lists
  `results/2026-10-18/` and `results/2026-10-18-name-first/`.
- **Committed results:**
  - Regenerate both `report.md` files with the new report. They are legacy
    runs, so the rates are unchanged and the CIs are new.
  - Correct `results/2026-10-18/DECISION.md`: remove "mistral.rs … slightly
    behind llama-server" (72.0 vs 76.7 is within noise), and add the CIs for
    the headline comparison (native − name-first grammar correct choice
    +11.0 pts [2.0–20.3]).
  - Mirror the correction in the aivyx-ecosystem ROADMAP.

## Testing

- **Header and run ids:**
  - The header is written first, with every field present.
  - `run_id` appears on every reply and status line, and `run_end` is
    written last.
  - Legacy files read as one run each.
  - `rescore` keeps run ids and adds `rescored_by`.
- **Report rows and verdict:**
  - Two runs of one endpoint give two rows, and the verdict is Undecided
    until `--run` or `--latest` is used.
  - `--latest` picks the newest run.
- **Bootstrap:**
  - Fixed-seed determinism.
  - A 100 % rate gives `[100.0–100.0]`.
  - A known small example falls in the expected range.
  - The paired difference is signed correctly.
- **Verdict wording:** *clearly* and *within noise* on both sides of a bar,
  and of the 5-point edge.
- **Temperature:**
  - It is sent only when set.
  - `[defaults]` applies, the endpoint value wins over it, and `extra_body`
    wins over both.
  - The header records the effective value, and the mismatch warning fires.
- **Capture:**
  - Each redaction kind, with counts in `source`.
  - Flag detection.
  - Not saved without `--accept` when stdin is not a terminal.
  - A non-loopback `--listen` is refused without `--allow-remote`.
- **Rendering:** `|` is escaped, the extra-calls column is present, and
  `report --out` creates its directory.
- **CI:** clippy `-D warnings`, fmt, and the new `cargo deny` job green.

## Order

1. Run header and metadata.
2. Report rows, CIs and verdict wording.
3. Temperature.
4. Capture guard.
5. CI, Dependabot and docs.
6. Regenerated reports and the DECISION.md / ROADMAP correction.

## Out of scope

- Changing the verdict rule or thresholds.
- Re-running any model.
- Scoring multiple tool calls beyond counting them.
- Per-request sampling seeds.
