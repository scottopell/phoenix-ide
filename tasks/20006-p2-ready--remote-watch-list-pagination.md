# Make remote watch listings complete or explicitly pageable

`ListWatchedConversations::run_routed` asks `remote_subscriptions(Some(peer.instance_id()), 1024)` for a selected peer. `Database::remote_subscriptions` applies a `LIMIT` without continuation or a truncation indicator, so with 1,025 stored subscriptions the tool succeeds while omitting at least one. REQ-FED-009 says the remote selection lists locally stored durable subscriptions for that peer; the current response cannot distinguish a complete list from a truncated one.

Add a bounded, deterministic paging contract (or another complete-list approach with an explicit tested bound) that does not misrepresent a partial result as complete. Exercise the 1,024/1,025 boundary through the Coordinator registration and preserve local watch behavior and peer isolation. Check the separate owner-facing subscription list for the same limit before deciding its scope.
