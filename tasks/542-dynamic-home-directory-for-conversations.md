# Task 542: Use Dynamic Home Directory for New Conversations

**Status**: Closed
**Priority**: Medium
**Created**: 2026-02-11

## Problem

New conversations currently default to a hard-coded working directory (likely `/home/exedev` or similar), which doesn't work correctly across different environments and user setups. This causes issues when:

- Running in containers with different user names
- Deploying to different environments (dev, prod, workspaces)
- Users with different home directory paths

## Goal

Default new conversations to use the dynamic home directory of the current user instead of hard-coded paths.

## Proposed Solution

1. **Detect current user's home directory** at runtime using environment variables or system APIs
   - Use `std::env::var("HOME")` or `dirs::home_dir()` in Rust
   - Fall back to reasonable defaults if detection fails

2. **Update conversation creation logic** to use dynamic path
   - Likely in conversation creation/initialization code
   - Should respect user-provided CWD if explicitly set

3. **Consider sensible fallbacks**:
   - Primary: User's actual home directory
   - Secondary: Current working directory where Phoenix was launched
   - Tertiary: `/tmp` or another safe default

## Implementation Notes

- Look for where `cwd` is set during conversation creation
- Check for any hardcoded paths like `/home/exedev`, `/home/bits`, etc.
- Ensure this works across all deployment modes (dev, prod, local)
- Test in different environments (native, Lima VM, containers)

## Success Criteria

- [ ] New conversations default to `$HOME` or equivalent
- [ ] No hardcoded user-specific paths in conversation defaults
- [ ] Works correctly in dev, prod-local, and VM deployments
- [ ] Existing conversations are not affected
- [ ] User can still explicitly set CWD if desired

## Related

- Production deployment work (tasks/ai-gateway-auth-deployment branch)
- Multi-environment support improvements
