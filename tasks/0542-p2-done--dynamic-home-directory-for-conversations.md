---
created: 2026-02-11
priority: p2
status: done
---

# Task 542: Use Dynamic Home Directory for New Conversations

## Summary

Default new conversations to use the dynamic home directory of the current user instead of hard-coded paths.

## Context

New conversations currently default to a hard-coded working directory (likely `/home/exedev` or similar), which doesn't work correctly across different environments and user setups. This causes issues when:

- Running in containers with different user names
- Deploying to different environments (dev, prod, workspaces)
- Users with different home directory paths

## Acceptance Criteria

- [ ] New conversations default to `$HOME` or equivalent
- [ ] No hardcoded user-specific paths in conversation defaults
- [ ] Works correctly in dev, prod-local, and VM deployments
- [ ] Existing conversations are not affected
- [ ] User can still explicitly set CWD if desired

## Notes

- Related to production deployment work (ai-gateway-auth-deployment branch)
