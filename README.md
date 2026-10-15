# keywords — fuzzy keyword correction for Spokenly

Handy's custom-words matcher (Levenshtein + Double Metaphone, 1–3 gram search)
as a tiny CLI that plugs into Spokenly's Bash Scripts pipeline (stdin → stdout).

Fast: ~3ms per dictation including process startup. No dependencies at runtime.

## Install

```bash
mkdir -p ~/.local/bin
curl -fsSL https://raw.githubusercontent.com/nub236/keywords/main/keywords-darwin-arm64 -o ~/.local/bin/keywords
chmod +x ~/.local/bin/keywords
```

To build from source instead:

```bash
cargo build --release
cp target/release/keywords ~/.local/bin/keywords
```

## Manage keywords

```bash
keywords manager            # interactive: list / add / remove / test / threshold / quit
keywords add "ChargeBee" "FlitePath" "MyTerm"
keywords remove "MyTerm"
keywords list
keywords threshold          # show (default 0.18, lower = stricter)
keywords threshold 0.25    # loosen if mishearings are missed
keywords test "the flight path is clear"   # dry-run: shows in -> out
keywords config-path        # where keywords are stored (~/.config/keywords/keywords.json)
```

Env overrides (optional): `KEYWORDS_FILE` (alternate store),
`KEYWORDS_THRESHOLD` (per-run threshold).

## Spokenly setup

1. Spokenly → your Mode → Bash Scripts.
2. In **Pre-AI Script** (raw transcript in, corrected text out), put:

```bash
~/.local/bin/keywords fix
```

That's it — no AI Instructions needed. Leave them blank for pure
scriptable correction, or keep them and the corrected text feeds the LLM.

Contract details (from Spokenly docs): the script reads the transcript from
stdin and prints the result to stdout. `keywords fix` with no args does
exactly that; `echo "hi" | keywords` also works. Must finish in 30s —
this finishes in milliseconds.

## How matching works

Ported from Handy's `src-tauri/src/audio_toolkit/text.rs`:

- Normalizes candidates (lowercase, strip non-alphanumerics) so
  "Charge B" matches "ChargeBee" and "Chat G P T" matches "ChatGPT".
- Scores with normalized Levenshtein distance, boosted (×0.3, ×0.2 for long
  codes) on exact Double Metaphone agreement — so "flight path" reaches
  "FlitePath" and "high tech" reaches "HiTek", while "flat pack" and
  "high tea" are left alone.
- Length gate (±25%, ±50% behind phonetic agreement) blocks things like
  "openaigpt" → "openai".
- Preserves your entry's casing (`FlitePath`, `SQLAlchemy`, `iPhone`) unless
  the whole span is shouted (`FLIGHT PATH` → `FLITEPATH`).
- Handles `R&D` ↔ "R and D" and compounds like `flightpath.com` → `FlitePath.com`.
- ASCII-only by design (same as Handy); CJK terms are skipped.
