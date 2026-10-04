# Check physical FTS text versus source freshness fingerprints

Captured during benchmark PR837 review r4175641508. Existing is_fresh_for compares typed source hashes to locator content_hash and checks physical presence, not physical FTS text equality. Assess REQ-RET-008 reconciliation authority against a deliberately corrupted physical row with current locator hash. Decide whether production freshness must detect it, then bound regression/implementation under that domain contract. Benchmark stays read-only; no automatic repair or scan framework.

Also assess locator conversation/type/timestamp versus authoritative source row (PR837 r4176027064); benchmark does not certify arbitrary locator corruption independently of production freshness authority.
