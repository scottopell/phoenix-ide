**Yes—macOS ACLs can solve this without making your home directory broadly readable.** There’s one important nuance: they can grant traversal through your home, but cannot restrict that traversal to just the Phoenix log.

### Why there’s a permissions problem

The Agent runs as a dedicated account, `_dd-agent`. To open:

```text
/Users/scottopell/.phoenix-ide/prod.log
```

it needs these rights:

| Path | Required right | Current situation |
|---|---|---|
| `/Users/scottopell` | Traverse/search | **Blocked** by your home’s `750` permissions |
| `.phoenix-ide` | Traverse/search | Already allowed by `755` |
| `prod.log` | Read | Already allowed by `644` |

So the missing permission is **walking through your home directory**, not reading the log itself.

### What an ACL adds

macOS supports richer ACL entries alongside the usual Unix owner/group/other permissions. We could add one entry conceptually saying:

```text
Allow this specific Agent account to search/traverse this directory.
```

For a directory, **search** means “resolve a known child name.” It does **not** mean “list the directory’s contents.”

That would let the Agent open the known log path without granting it:
- a listing of your home;
- write access;
- membership in your user group;
- recursive permissions;
- Full Disk Access.

Your existing permissions and ACL entries would remain in place.

### The caveat: traversal isn’t a one-file tunnel

Once `_dd-agent` can traverse your home, it could also access **other known paths whose remaining directories and files already permit access**.

For example, another `644` file directly inside your home could become readable if the Agent knows its name. A file protected with `600`, or behind another inaccessible directory, remains protected.

So the accurate claim is:

> **A small, targeted permission change—not access exclusively to one log.**

Also, the log-filter rules control **what gets exported**. They do not prevent the Agent process from reading the raw source file locally.

### My recommendation

For this setup, **a search-only ACL is probably the simplest reasonable solution**. It is narrower than running the Agent as root and avoids adding another background process just to copy logs elsewhere.

Before applying it, I’ll:
1. Verify the actual installed Agent identity.
2. Record the existing ACL.
3. Add only the missing search permission, without inheritance.
4. Test that the Agent can read the log but cannot list your home.
5. Record precisely how to remove our entry without disturbing preexisting permissions.

A redacted log stream outside your home would provide stronger separation, but we haven’t identified an existing supported handoff that gives us that without extra plumbing.

**No ACL has been changed yet.** This is ordinary macOS filesystem access control; it does not require disabling SIP or granting a broad privacy permission.
