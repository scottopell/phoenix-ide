# TmuxWindow restart-surviving wake adapter

Depends on tasks 44011, 44012, and 44013 and their merged PRs.

Add `TmuxWindow` only after the authoritative aggregate, Bash adapter, and public projections are stable. Use durable tmux server token/window identity and completion-marker recovery to prove restart-surviving success. Adapter observation and cleanup must be token-fenced and cannot introduce parallel lifecycle, cancellation, receipt, delivery, or SSE authorities.

This is a separate later PR. Do not add tmux wake support to the Bash slice.
