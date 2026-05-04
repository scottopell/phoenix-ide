---
created: 2026-05-01
priority: p1
status: in-progress
artifact: pending
---

# mobile-chain-awareness-sequential-slugs

## Plan

# Mobile Chain Awareness, Chain Member Display, and Sequential Continuation Slugs

## Context

Three compounding problems make the conversation list hard to navigate, especially on mobile:

1. **Mobile has zero chain grouping.** `groupedItems` is gated on `sidebarMode`, which is always false on mobile. The full-page conversation list renders flat — no chain headers, no grouping, no visual relationship between continuations.

2. **Chain member rows show raw slugs.** Inside a desktop chain block, each member still renders its full raw slug. The chain header already shows the clean chain name, so the member slug adds noise rather than signal.

3. **Continuation slugs compound with every level.** The backend appends `-continued-{uuid8}` to the *parent's* slug (not the root's), producing:
   ```
   my-task
   my-task-continued-94afa354
   my-task-continued-94afa354-continued-ab1ab4c0
   my-task-continued-94afa354-continued-ab1ab4c0-continued-b8395d08
   ```
   Observed 7-level chains with 140+ character slugs in production. The UUID was added to ensure INSERT uniqueness before the TOCTOU UPDATE-arbitration could run — but this is the same collision problem the regular conversation-creation code already solves with a retry loop on SQLite error `2067`.

---

## Changes

### 1. Backend: sequential continuation slugs (`src/db.rs`)

Replace the UUID-prefix approach with a sequential counter and retry-on-collision, matching the existing pattern in `create_conversation()` (lines 396–436).

**New logic in `continue_conversation()`:**

```rust
// Before the transaction:
// 1. Walk to chain root (or use parent_id if parent IS the root)
let root_id = self.chain_root_of(parent_id).await?
    .unwrap_or_else(|| parent_id.to_string());
let root = self.get_conversation(&root_id).await?;
let chain_len = self.chain_members_forward(&root_id).await?.len();

// 2. Base slug: {root_slug}-{N} where N = chain_len + 1 (e.g., root only → new is #2)
let root_slug = root.slug.as_deref().unwrap_or("conversation");
let base_n = chain_len + 1;
let mut candidate_slug = format!("{root_slug}-{base_n}");
let mut offset: usize = 0;

// Inside the transaction, retry on UNIQUE violation (SQLite error 2067):
loop {
    match sqlx::query(INSERT_SQL).bind(&candidate_slug)...execute(&mut *tx).await {
        Ok(_) => break,
        Err(sqlx::Error::Database(ref e)) if e.code().as_deref() == Some("2067") => {
            offset += 1;
            if offset > 20 {
                // Safety valve: fall back to UUID suffix (same as create_conversation)
                let uid = uuid::Uuid::new_v4().to_string();
                candidate_slug = format!("{root_slug}-{}-{}", base_n, uid.get(..8).unwrap_or(&uid));
            } else {
                candidate_slug = format!("{root_slug}-{}", base_n + offset);
            }
        }
        Err(e) => return Err(DbError::Sqlx(e)),
    }
}
// Rest of UPDATE arbitration + commit unchanged.
```

**Result:** `my-task` → `my-task-2` → `my-task-3`. Concurrent continuations still handled correctly: the UNIQUE violation (from a concurrent INSERT winning the slug) triggers the retry path; `rows_affected() == 0` on the UPDATE (from a concurrent winner that beat us in the arbitration check) triggers the existing AlreadyContinued rollback path.

Remove the now-dead `new_id_prefix` variable and its comment block.

### 2. UI: enable chain grouping in full-page (mobile) mode (`ui/src/components/ConversationList.tsx`)

```tsx
// Before
const groupedItems = useMemo(() => {
  if (!sidebarMode || showArchived) return null;
  ...

// After
const groupedItems = useMemo(() => {
  if (showArchived) return null;
  ...
```

Chain blocks and their CSS (`.conv-chain-block`, `.conv-chain-header`, `.conv-chain-members`, etc.) are not scoped to `.sidebar-mode` — they'll render correctly in full-page mode with the additional CSS added below.

### 3. UI: position label for chain member rows (`ui/src/components/ConversationList.tsx`)

Pass index and total from `renderChainBlock` to `renderConvRow`:

```tsx
// In renderChainBlock:
{item.members.map((m, idx) =>
  renderConvRow(m, {
    isChainMember: true,
    isChainLatest: m.id === item.latestMemberId,
    chainIndex: idx,
    chainTotal: item.members.length,
  }),
)}
```

In `renderConvRow`, when `chainIndex` is provided, replace the raw slug display with `#{chainIndex + 1}` and move the raw slug to a `title` attribute (hover tooltip):

```tsx
// slug span — show position in chain if inside a chain block
const slugDisplay = chainIndex !== undefined
  ? `#${chainIndex + 1}`
  : conv.slug;
const slugTitle = chainIndex !== undefined ? conv.slug ?? undefined : undefined;

<span className="conv-item-slug-text" title={slugTitle}>
  {slugDisplay}
</span>
```

This means a 3-member chain block shows:
```
⊟ my-task [3]
  ● #1   [explore] 3 msgs
  ● #2   [work]    12 msgs
  ● #3   [work]    latest  7 msgs
```

### 4. CSS: chain blocks at full-page scale (`ui/src/index.css`)

Add rules for chain elements when NOT in sidebar mode. In full-page mode, chain blocks should match the surrounding list density (standard `conv-item` sizing) rather than the sidebar micro-scale (10–13px):

```css
/* Full-page (non-sidebar) chain block sizing */
#conversation-list:not(.sidebar-mode) .conv-chain-header {
  font-size: 14px;
  padding: 8px 10px;
}

#conversation-list:not(.sidebar-mode) .conv-chain-members {
  padding-left: 20px;
}

#conversation-list:not(.sidebar-mode) .conv-chain-count {
  font-size: 12px;
}
```

---

## Not in scope

**Migration of existing continuation slugs.** Existing prod conversations already have the `-continued-{uuid}` compound slugs. Renaming them in a migration requires walking chains, reconstructing sequential slugs, and handling conflicts — a non-trivial Rust data migration beyond the migration system's current SQL-only capability. The UI display fixes (chain grouping + position labels) mitigate the pain immediately. Slug migration tracked separately.

---

## Acceptance Criteria

- [ ] `my-task` continued once → slug is `my-task-2`, not `my-task-continued-{uuid}`
- [ ] `my-task-2` continued → slug is `my-task-3` (uses root slug, not parent slug)
- [ ] Concurrent continuation requests still safe: one wins with clean slug, other returns `AlreadyContinued`
- [ ] Mobile (375px): conversation list shows chain blocks with collapsible headers
- [ ] Chain member rows show `#1`, `#2`, `#3 latest` — not raw slugs; raw slug visible on hover
- [ ] Desktop sidebar chain display unchanged
- [ ] `./dev.py check` passes (clippy, fmt, tests, codegen)


## Progress

