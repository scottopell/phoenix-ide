---
created: 2026-04-29
priority: p1
status: done
artifact: src/db/migrations.rs
---

Add migration creating chain_qa table (UUID PK; root_conv_id with FK ON DELETE CASCADE to conversations(id); question, answer NULL, model, status enum {in_flight, completed, failed, abandoned}; snapshot_member_count and snapshot_total_messages INTEGER NOT NULL; created_at, completed_at NULL) plus index on (root_conv_id, created_at). Same migration adds nullable chain_name TEXT column to conversations. Update Conversation Rust struct to include chain_name. Update SELECT queries that load Conversation to project chain_name. Add Database methods chain_members_forward(root_id) and chain_root_of(conv_id) using recursive CTEs over continued_in_conv_id. Tests: migration runs cleanly on fresh and existing DB; chain walk correctness on linear chain, single conv, and broken chain.
