---
created: 2026-02-09
priority: p2
status: ready
---

# Handle Slug Collisions Gracefully

## Summary

When creating a conversation with a title that generates a duplicate slug, the API returns a 500 error instead of handling it gracefully.

## Problem

```
API Error: 500 - {"error":"Database error: UNIQUE constraint failed: conversations.slug"}
```

This happens when:
1. User creates conversation "Navigate to example.com"
2. LLM generates title → slug "navigate-example-website"
3. User creates another conversation with similar prompt
4. Same/similar title → same slug → UNIQUE constraint violation

## Solution

When inserting a conversation fails due to slug collision, append a random suffix:
- First try: `navigate-example-website`
- On conflict: `navigate-example-website-a7b3`

## Implementation

In `src/api/handlers.rs` or `src/db.rs`, catch the UNIQUE constraint error and retry with a suffix:

```rust
fn generate_unique_slug(base_slug: &str, conn: &Connection) -> String {
    let mut slug = base_slug.to_string();
    let mut attempts = 0;
    while attempts < 10 {
        // Check if slug exists
        let exists: bool = conn.query_row(
            "SELECT 1 FROM conversations WHERE slug = ?",
            [&slug],
            |_| Ok(true)
        ).unwrap_or(false);
        
        if !exists {
            return slug;
        }
        
        // Append random suffix
        let suffix: String = (0..4)
            .map(|_| char::from(b'a' + rand::random::<u8>() % 26))
            .collect();
        slug = format!("{}-{}", base_slug, suffix);
        attempts += 1;
    }
    // Fallback to UUID
    format!("{}-{}", base_slug, uuid::Uuid::new_v4().to_string().split('-').next().unwrap())
}
```

## Acceptance Criteria

- [ ] Duplicate slugs get random 4-char suffix appended
- [ ] No 500 errors from slug collisions
- [ ] Original slug preserved when possible (only add suffix on collision)
- [ ] Works for both LLM-generated titles and manual renames

## Files

- `src/api/handlers.rs` - `create_conversation` handler
- `src/db.rs` - possibly add helper function
