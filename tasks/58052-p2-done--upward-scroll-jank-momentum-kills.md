Upward scrolling is janky on iOS Safari while downward is smooth — three compounding physical-layer causes in `VirtualTranscript` / `MessageList`.

1. Momentum-killing scrollTop compensation (main cause, explains the up/down asymmetry). Rows mount with `estimatedExtent=120` px; real agent-turn rows are often several times taller. Scrolling up mounts never-measured rows above the top anchor; each measurement fires `applyPhysicalChange` → `applyAnchor` → a programmatic `scroller.scrollTop = …` write to keep the anchor fixed. On iOS Safari a programmatic scrollTop write stops momentum dead, so flick-scrolling up through unmeasured history is a series of momentum kills. Scrolling down, newly measured rows sit below the anchor, the anchor offset is unchanged, and `setScrollerScrollTop` skips the write when the value is equal — no kills, hence "down is mostly fine". (`overflow-anchor: none` is set, and iOS has no native anchoring anyway, so this manual scheme is the only anchoring — it needs to be momentum-aware, e.g. defer corrections while a momentum scroll is in flight, or absorb them into the top spacer instead of scrollTop.)

2. Extra O(n) work only on the upward path. Every upward scroll event and every upward touchmove calls `requestFromUpwardIntent`, which calls `physicalSnapshot()` → `synchronizedPhysicalSnapshot` → `recompute` → a full `buildTranscriptLayout` rebuild — *before* the `startIndex <= 2` guard is evaluated. Downward frames do one rebuild (VirtualTranscript's own `handleScroll`); upward frames do two to three. `recompute` also bumps `layoutRevision` as a side effect of what is conceptually a read. Cheap fix: consult the last published range (`firstVisibleUnitIndexRef`) before taking a synchronized snapshot.

3. Mid-gesture prefix expansion near the top. `onTouchMove` triggers `requestEarlierHistory('upward-intent')` while the finger is still down; the subsequent prefix insertion + `restore_after_prefix_expansion` scrollTop write lands during an active drag or momentum — another momentum kill precisely when the user reaches the top of loaded history.

Related polish: `.virtual-transcript` has no `overscroll-behavior: contain`, so hitting the transcript's top during a drag chains the gesture into the page/body rubber-band on iOS — the whole app bounces, compounding the perceived jank (and the top-overscroll frames feed the direction classifier, see task 58050).

## Resolution

Items 1, 2, and the `overscroll-behavior` polish are implemented: mid-scroll
anchor corrections are absorbed into the top spacer (drift) and reconciled
with one scrollTop write after scroll settle; `requestFromUpwardIntent` reads
the last published range instead of forcing a layout rebuild per event.

Residual (item 3): the `restore_after_prefix_expansion` positioning write is
still a direct scrollTop write at the moment older history is inserted. It
fires once per history-page boundary (not per frame) and intentionally keeps
the viewport stationary, so the momentum cost is a bounded pause at the
boundary rather than continuous jank. Deferring it behind the drift mechanism
would couple the positioning reducer to scroll-settle timing; revisit only if
the boundary pause is still noticeable on-device.
