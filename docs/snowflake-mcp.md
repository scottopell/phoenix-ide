# Hosted Snowflake MCP in Phoenix

Phoenix can connect directly to a Snowflake-managed MCP server using a
preconfigured public OAuth client. Replace the account and MCP server identifiers
below, then merge this entry into the `mcpServers` object in `~/.cursor/mcp.json`:

```json
{
  "snowflake": {
    "type": "http",
    "url": "https://your-account.snowflakecomputing.com/api/v2/databases/YOUR_DATABASE/schemas/YOUR_SCHEMA/mcp-servers/YOUR_MCP_SERVER",
    "oauth": {
      "clientId": "LOCAL_APPLICATION",
      "callbackUrl": "http://127.0.0.1:53682/",
      "scopes": "session:role:all refresh_token"
    }
  }
}
```

`callbackUrl` is Phoenix's explicit callback setting. This example is for Phoenix
reading Cursor's configuration file; support for the setting in Cursor itself
is not asserted. Reload Phoenix's MCP configuration, then complete Snowflake's
sign-in prompt in a browser on the same computer as Phoenix. Phoenix keeps
its own tokens; it does not reuse the ChatGPT plugin's saved authorization.

Use an available local port. The callback must use HTTP and name `localhost`,
`127.0.0.1`, or `[::1]`. Credentials, query strings, fragments, whitespace,
control characters, backslashes, and port zero are rejected. An omitted port
resolves to HTTP's default port 80. Phoenix must bind the callback address
before it offers a sign-in URL. No public endpoint or tunnel is needed.

Do not combine `callbackUrl` with `callbackPort`. A `callbackPort` entry uses
`http://localhost:<port>/callback`, including the Slack configuration with port
3118. Omitting both settings uses Phoenix's normal callback route. An explicit
URL requires a nonempty `clientId`; no client secret is required for this public
client flow.

To verify the connection, request your current identity and a small table read:

```sql
SELECT CURRENT_USER(), CURRENT_ROLE();
```

```sql
SELECT * FROM YOUR_DATABASE.YOUR_SCHEMA.YOUR_TABLE LIMIT 1;
```

Full sign-in, MCP requests, and token renewal need live verification for your
Snowflake account after installing the Phoenix build. Availability and
permissions of the built-in application are controlled by the Snowflake account.

See [Snowflake's local application OAuth guide](https://docs.snowflake.com/en/user-guide/oauth-local-applications)
and [ADR-251](../specs/adrs/251_explicit-loopback-oauth-callback-urls.md).
