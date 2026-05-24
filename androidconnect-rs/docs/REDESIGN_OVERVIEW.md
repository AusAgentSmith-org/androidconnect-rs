# Desktop redesign — outstanding work index

The B/C/D stages of the Fluent 2 redesign shipped, but a number of items
from the original critique were missed. This index points to the five
passes that close those gaps.

Each pass is self-contained — pick one up and execute it without needing
the others. Pass A is the cheapest and most overdue; do it first.

| Pass | Plan | Effort | Theme |
|---|---|---|---|
| [A](REDESIGN_PASS_A.md) | Fundamentals (overdue cleanup) | 1 day | Token sweep, Card uniformity, hover states, animated heartbeat, working TopBar buttons, banner action, refreshing timestamps |
| [B](REDESIGN_PASS_B.md) | Biggest visible wins | 1 day | Phone illustration, Mirror capability card, Mirror auto-hide chrome, Skeleton adoption |
| [C](REDESIGN_PASS_C.md) | Structurally meaningful | 2 days | Files Grid + breadcrumb + selection footer, notification grouping, message date separators, Enter-to-send, double-click in Files, brand colours, Pair countdown + copy, DeviceChip popover |
| [D](REDESIGN_PASS_D.md) | Protocol-touching (Storage) | 1 day | StorageBreakdown payload, Android-side wire-up, Overview Storage card |
| [E](REDESIGN_PASS_E.md) | Framework follow-ups | parking lot | Real GPUI `letter_spacing`, Metal/HLSL backdrop blur, dual-filter Gaussian, Mica on Windows |

## Design source of truth

`/tmp/design-desktop/design-docs/` (uploaded into the repo as
`design-docs/` if it isn't there already). The design README's
implementation order roughly maps to Passes A → B → C → D.

## How to run a pass

1. Read the pass's `REDESIGN_PASS_*.md` end to end.
2. Run `cargo fmt && cargo clippy -- -D warnings && cargo test` to confirm
   you're starting from green.
3. Implement the items in order — they're listed in a sensible build
   sequence (later items often depend on earlier ones).
4. Walk through the Verification section at the bottom of the pass plan.
5. `cargo fmt && cargo clippy -- -D warnings && cargo test` again.
6. Commit (one commit per pass is fine, or split if it's natural) and push.
   CI must go green on each push.
