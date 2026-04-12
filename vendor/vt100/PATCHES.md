# Local patches to vt100 0.15.2

Source: https://crates.io/crates/vt100/0.15.2
Reason: upstream has panics on edge-case input (tracked in task 08667)

## Patch 1 — src/grid.rs: saturating_sub in col_wrap (line 672)

**Problem:** `prev_pos.row -= scrolled` overflows (u16) when `scrolled > prev_pos.row`.
Triggered at 1×1 terminal size.

**Fix:** `prev_pos.row = prev_pos.row.saturating_sub(scrolled);`

## Patch 2 — src/grid.rs: guard against width > size.cols in col_wrap (line 668)

**Problem:** `self.size.cols - width` overflows when a wide char (width=2) is placed
in a terminal narrower than the char's display width.

**Fix:** Added `width <= self.size.cols &&` guard before the subtraction.

## Patch 3 — src/screen.rs: clamp width at entry (line 822)

**Problem:** Multiple downstream arithmetic expressions assume `width <= size.cols`.
When a wide char (width=2) arrives on a ≤1-col terminal, all expressions of the
form `size.cols - width` overflow.

**Fix:** After computing `width`, clamp it to `size.cols.max(1)` so all downstream
arithmetic is safe. This causes wide chars to be treated as width=1 on terminals
too narrow to hold them, which is the least-bad rendering choice.

## Patch 4 — src/screen.rs: guard in wide-char wrap check (line 837)

**Problem:** `pos.col > size.cols - width` overflows when `width > size.cols`.

**Fix:** Added `size.cols > 0 && width <= size.cols &&` guard.

## Patch 5 — src/screen.rs: early-return guard in text() (new)

**Problem:** `text()` has many downstream paths that assume `size.rows > 0` and
`size.cols > 0`. If either is zero, arithmetic and cell lookups panic.

**Fix:** Return early at the start of `text()` when `size.rows == 0 || size.cols == 0`.

## Patch 6 — src/screen.rs: if-let in wide-char drawing path

**Problem:** The wide/normal character drawing section uses `.unwrap()` on
`drawing_cell_mut()` calls that access `pos.col + 1` and `pos.col - 1`. These
return `None` when the adjacent column doesn't exist (e.g. wide char metadata
referencing a column past the terminal width after a resize or unusual input).

**Fix:** Replaced all `.unwrap()` calls in the `else { ... }` branch of `text()`
with `if let Some(...)` guards. Missing cells are silently skipped rather than panicking.

---

To remove this patch: delete `vendor/vt100/` and the `[patch.crates-io]` block in
the root `Cargo.toml`. The bugs exist in vt100 0.16.2 as well; wait for an upstream
fix or submit a PR to https://github.com/doy/vt100-rust.
