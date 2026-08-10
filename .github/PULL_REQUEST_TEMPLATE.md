## What this changes

## Why

## Verification
- [ ] `cargo build --workspace` green
- [ ] `cargo test --workspace` green
- [ ] Verified by hand: <!-- what you actually ran/clicked/played and observed -->
- [ ] UI change? Screenshot attached, values traced to design tokens
- [ ] New `Operation` variant? Schema guard acknowledged; confirmation broker updated if destructive

## Ground rules check
I confirm this change keeps all mutations on the Operation path, keeps
`apply()` pure, keeps time in integer frames, and doesn't block the UI thread
(see CONTRIBUTING.md).
