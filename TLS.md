# Phoenix TLS

Phoenix can serve HTTPS directly from the Rust binary. TLS is opt-in; the
default remains plain HTTP because most deployments do not need browser HTTP/2
or certificate management.

HTTPS matters for Phoenix because browsers only negotiate HTTP/2 with TLS via
ALPN. They do not use h2c for normal web apps. When TLS is enabled, Phoenix
advertises both `h2` and `http/1.1`, so browsers can multiplex many SSE streams
over one connection while older clients can keep using HTTP/1.1.

## Current Model

There are two TLS modes:

- `auto`: Phoenix owns a local private CA and issues a leaf certificate at
  startup.
- `manual`: Phoenix reads the exact cert/key paths configured in the
  environment.

The recommended remote-host workflow uses both:

1. Keep one Phoenix private CA on the machine where you issue certs.
2. Trust that CA once on each browser machine.
3. Issue a per-host leaf certificate bundle for each Phoenix host.
4. Copy only the leaf bundle to the remote host.
5. Install the bundle on the remote host, which configures manual TLS.

The CA private key should not be copied to remote Phoenix hosts. A remote host
only needs its own server certificate and server private key.

## Trust Model

Browser trust has two separate checks:

- The certificate chain must end at a CA the browser trusts.
- The URL hostname must appear in the leaf certificate's Subject Alternative
  Name list.

For `https://localhost:<port>`, the certificate must include `localhost` or the
loopback IP address you use. Phoenix includes `localhost`, `127.0.0.1`, and
`::1` by default for auto-issued certificates and for `./dev.py tls issue`.

For `https://phoenix-host.internal:8031`, the certificate must include
`phoenix-host.internal`. It does not matter whether the hostname comes from
public DNS, private DNS, `/etc/hosts`, or a VPN-only resolver; browser TLS
validation still requires a trusted issuer and a SAN matching the hostname.
Public ACME/Let's Encrypt is not part of Phoenix's built in flow for
private/internal hostnames.

Trusting the Phoenix CA means the browser machine will trust leaf certificates
issued by that CA. It does not give remote hosts the ability to issue new certs
unless you copy them the CA private key. Do not do that for the normal flow.

## Development HTTPS

Use this when you need to reproduce browser HTTP/2 behavior locally:

```bash
./dev.py up --https
```

or, if Phoenix is already built and you only need to restart the backend:

```bash
./dev.py restart --https
```

This enables `PHOENIX_TLS=auto` for the dev server unless an explicit TLS config
already exists. The default CA directory is:

```text
~/.phoenix-ide/tls
```

The dev output prints the direct HTTPS URL, for example:

```text
Direct UI: https://localhost:8033
```

Vite still serves the dev UI over HTTP. When Phoenix is in HTTPS mode, `dev.py`
configures Vite's API proxy to talk to Phoenix over HTTPS and disables proxy
certificate verification for that local dev proxy. The embedded Phoenix UI is
also available directly over HTTPS, which is the route to use when validating
browser ALPN/HTTP/2 behavior.

If the browser shows `ERR_CERT_AUTHORITY_INVALID`, the server is working but the
browser machine does not yet trust the Phoenix CA.

## One-Time CA Setup

Create or show the local Phoenix CA:

```bash
./dev.py tls ca
```

The output names two files:

```text
cert=/Users/<you>/.phoenix-ide/tls/phoenix-local-ca.pem
key=/Users/<you>/.phoenix-ide/tls/phoenix-local-ca-key.pem
```

Trust the `cert` file on browser machines. Keep the `key` file private on the
machine that issues certificates.

Phoenix does not automatically mutate OS trust stores. Trust installation is a
manual OS/browser action:

- macOS: trust `~/.phoenix-ide/tls/phoenix-local-ca.pem` in the system or
  browser trust store.
- Linux: install the CA cert into the distro/browser trust store used by the
  browser you run.
- Fresh machines: copy only `phoenix-local-ca.pem` to the browser machine for
  trust. Do not copy `phoenix-local-ca-key.pem` unless that machine is meant to
  issue certificates.

## Remote Production Flow

Assume:

- You SSH to the remote machine as `ssh-host`.
- You open Phoenix in the browser as `https://phoenix-host.internal:8031`.

These may be the same string, but they do not have to be. The certificate must
use the browser hostname:

```text
phoenix-host.internal
```

On the machine that owns the CA:

```bash
./dev.py tls issue phoenix-host.internal
```

This writes:

```text
~/.phoenix-ide/tls-bundles/phoenix-host.internal.tar.gz
```

The bundle contains:

```text
server.pem
server-key.pem
phoenix-tls.json
```

It does not contain `phoenix-local-ca-key.pem`.

Copy the bundle to the remote host:

```bash
scp ~/.phoenix-ide/tls-bundles/phoenix-host.internal.tar.gz ssh-host:~/
```

On the remote host, from that host's Phoenix repo checkout:

```bash
./dev.py tls install ~/phoenix-host.internal.tar.gz
./dev.py prod deploy
```

`tls install` copies the leaf cert/key into:

```text
~/.phoenix-ide/tls/<host>.pem
~/.phoenix-ide/tls/<host>-key.pem
```

It also updates the repo-local `.phoenix-ide.env`:

```env
PHOENIX_TLS=manual
PHOENIX_TLS_CERT_PATH=/home/<you>/.phoenix-ide/tls/phoenix-host.internal.pem
PHOENIX_TLS_KEY_PATH=/home/<you>/.phoenix-ide/tls/phoenix-host.internal-key.pem
PHOENIX_PUBLIC_URL=https://phoenix-host.internal:8031
```

`./dev.py prod deploy` reads `.phoenix-ide.env`, installs the env into the
production service environment where needed, and starts Phoenix on:

```text
https://phoenix-host.internal:8031
```

This matches the intended remote workflow: keep a git checkout on the remote
host and run `./dev.py prod deploy` locally on that host.

## Extra Hostnames

If one host needs additional SANs, pass repeated `--host` flags when issuing:

```bash
./dev.py tls issue phoenix-host.internal \
  --host phoenix-host \
  --host 10.0.0.12
```

The primary positional hostname is always included. `localhost`, `127.0.0.1`,
and `::1` are also included by default.

For auto mode without `dev.py tls issue`, use `PHOENIX_TLS_HOSTS`:

```env
PHOENIX_TLS=auto
PHOENIX_TLS_HOSTS=phoenix-host.internal,phoenix-host,10.0.0.12
```

## Environment Variables

| Variable | Meaning |
|----------|---------|
| `PHOENIX_TLS` | `auto`, `on`, `true`, or `1` enables managed local CA mode. `manual` requires explicit cert/key paths. `off`, `none`, `false`, `0`, or unset disables TLS unless manual cert/key paths are both set. |
| `PHOENIX_TLS_HOSTS` | Comma-separated extra SANs for `PHOENIX_TLS=auto`. Phoenix always includes `localhost`, `127.0.0.1`, and `::1`. |
| `PHOENIX_TLS_DIR` | Directory for the managed CA and auto-issued leaf cert. Defaults to the parent of `PHOENIX_DB_PATH` plus `/tls`; with the usual paths this is `~/.phoenix-ide/tls`. |
| `PHOENIX_TLS_CERT_PATH` | Manual server certificate PEM path. If both cert and key paths are set, Phoenix uses manual TLS even if `PHOENIX_TLS` is unset. |
| `PHOENIX_TLS_KEY_PATH` | Manual server private key PEM path. Required with `PHOENIX_TLS_CERT_PATH`. |
| `PHOENIX_PUBLIC_URL` | Display URL used by `./dev.py prod deploy` and `./dev.py prod status`. The Rust server does not read it. |
| `VITE_API_SCHEME` | Dev-only Vite proxy scheme. `./dev.py up --https` sets this to `https`. |
| `VITE_API_PROXY_SECURE` | Dev-only Vite proxy certificate verification toggle. `./dev.py up --https` sets this to `false` for the local private CA. |

Manual cert/key path pair wins over `PHOENIX_TLS=auto`. Setting only one of
`PHOENIX_TLS_CERT_PATH` or `PHOENIX_TLS_KEY_PATH` is a startup error.

## Files and Lifetimes

Managed CA files:

```text
phoenix-local-ca.pem
phoenix-local-ca-key.pem
```

Auto-issued local server files:

```text
phoenix-local-server.pem
phoenix-local-server-key.pem
```

Generated cert validity:

- CA: 3650 days.
- Leaf/server certs: 397 days.

`PHOENIX_TLS=auto` reissues the leaf certificate on startup using the existing
CA. It does not rotate the CA automatically.

For remote `manual` deployments, rotate by issuing and installing a new bundle:

```bash
./dev.py tls issue phoenix-host.internal
scp ~/.phoenix-ide/tls-bundles/phoenix-host.internal.tar.gz ssh-host:~/
ssh ssh-host 'cd ~/phoenix-ide && ./dev.py tls install ~/phoenix-host.internal.tar.gz && ./dev.py prod deploy'
```

## Verification

Protocol checks:

```bash
curl -k -D - https://localhost:8033/version
curl -k --http1.1 -D - https://localhost:8033/version
openssl s_client -connect localhost:8033 -servername localhost -alpn h2 < /dev/null
```

Expected signals:

- Default curl should show `HTTP/2 200` when it supports HTTP/2.
- `curl --http1.1` should show `HTTP/1.1 200 OK`.
- `openssl s_client` should report `ALPN protocol: h2`.

Browser checks:

1. Trust `phoenix-local-ca.pem` on the browser machine.
2. Open `https://localhost:<port>` or `https://<host>:8031`.
3. Confirm the page loads without a certificate warning.
4. In browser devtools, confirm requests use `h2` if the protocol column is
   enabled.

Before the CA is trusted, browser failure with `ERR_CERT_AUTHORITY_INVALID` is
expected.

## Security Notes

- TLS is opt-in. Do not enable it unless browser HTTP/2 or HTTPS semantics are
  useful for the deployment.
- Do not reuse one leaf private key across all Phoenix hosts.
- Do not copy the CA private key to hosts that only serve Phoenix.
- Do trust the CA only on machines where you are comfortable trusting Phoenix
  certificates.
- This flow is intended for single-user/internal-DNS deployments, not public
  multi-user web hosting.
- If Phoenix is exposed beyond a private network/VPN, revisit authentication,
  DNS, firewalling, and public certificate management separately.
