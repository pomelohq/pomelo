# Security

Pomelo is a developer tool that runs on your own machine (or a Linux host you own) as your own user. It
starts your services, shells and coding agents, so anything that can drive Pomelo can run code as you. This
document says which surfaces Pomelo exposes, who can reach each one, what it trusts, and what it does not try
to defend against. Crate paths are under `rust/crates/`.

## Reporting a vulnerability

Please report security issues privately through GitHub: on https://github.com/pomelohq/pomelo open the
Security tab and choose "Report a vulnerability" (a private security advisory). Do not open a public issue
for a suspected vulnerability.

Include the version (`pom --version`), the platform, and the smallest steps that show the problem. We will acknowledge the report and ship a fix in a patch release, crediting you in the advisory
unless you ask otherwise.

## Supported versions

Only the latest release gets security fixes. The app updates itself (macOS, and Linux installed with install.sh) and `pom` on a host can be
updated from the app, so the fix for an issue is the next release, not a backport.

## Summary

| Surface | Reachable by | Trust assumption | Main controls |
| --- | --- | --- | --- |
| Dev proxy, loopback bind (default) | Local processes, and web pages in your browser | Your own processes are trusted; web pages are not | Host allowlist, reserved `/_pom/*`, cross-site checks |
| Dev proxy, non-loopback bind or a public `proxy.domain` | Anyone who can reach the address, or the domain through a reverse proxy | Only holders of the proxy token | Refuses to start without a token unless `allow_unauthenticated`; token by header, cookie or one-time query |
| Webhook relay | Webhook providers, usually through a tunnel | Payload signatures are your app's job; `/_pom/github` checks its own | Token for non-local peers, signatures passed through unchanged, HMAC on `/_pom/github` |
| Agent control (hooks, `pom agent`, MCP) | Processes running as you, including agents | Agents are guarded against mistakes, not against a hostile process of your own user | Hooks wired by pom, fail-closed policy, per-workspace gate, rate limits |
| Remote (`pom server`) | Your own user on the host, over ssh | ssh is the authentication | Private Unix socket, peer uid check, no network port, path guards |
| Secrets | Your own user | The machine is yours | AES-256-GCM at rest, `0600` files, values never shown to agents |
| Updates | GitHub releases | The release signing key and the GitHub release | EdDSA signature on every download the updaters install (macOS, Linux, host pom) |
| Local processes (PTY holders) | Your own user | Unix permissions | `0700` socket folder, `0600` sockets |

Non-goals for every surface: defending against malware already running as your user, against root, or
against a project whose own code (its `pom.yml` commands, setup steps, dependencies) is hostile. Opening a
project or importing an archive means you trust what it runs.

## Dev proxy

`pom_proxy` serves every workspace's services under names like `web.feat-login.myproject.localhost` and
listens on `proxy.bind` (loopback by default).

**Loopback bind.** Any process on the machine can connect, and so can any web page open in your browser
(a page can make requests to `localhost`). The proxy defends against the browser cases:

- DNS rebinding: requests whose `Host` is not `localhost`, `*.localhost`, the configured `proxy.domain` and
  its subdomains, a loopback IP, or the bind IP are answered `421` before anything else
  (`pom_proxy/src/access.rs`, `host_allowed`).
- `/_pom/*` is reserved for pom and never routed to a service. A cross-site browser request
  (`Sec-Fetch-Site: cross-site|same-site`, or an `Origin` that is not the request's host, or `Origin: null`)
  gets `403`; the only state-changing endpoint, the fault-rule reload, also requires `POST`, the
  `x-pom-control: faults` header and a request from this machine (see "Local" below)
  (`pom_proxy/src/server.rs`, `handle_control`).

Traffic to your own services is only filtered by the Host allowlist. A page you visit can still send the
simple cross-site requests any browser allows to a local app; protecting your app against CSRF is the app's
job, exactly as without pom.

**Bound beyond loopback** (a LAN, VPN or tailnet address, or `0.0.0.0`), **or serving a public domain**
(`proxy.domain` other than `localhost`, which a reverse proxy on the same machine may forward to loopback). The
proxy refuses to start unless
a token is configured (the env var named by `proxy.token_env`, default `POM_PROXY_TOKEN`, or one stored
encrypted with `pom proxy token new`), or you set `allow_unauthenticated true` to accept the risk
(`refusal` in `access.rs`). A token must be at least 16 characters from `[A-Za-z0-9._~-]`. A peer that is not
local must present it in the `X-Pom-Token` header or the `pom_token` cookie; a `GET` with `?pom_token=` is
redirected to the same URL without it and sets an `HttpOnly`, `SameSite=Lax` cookie (`Secure` under https).
Comparison is constant time, and the token is stripped from headers, cookies and query strings before a
request is forwarded or logged. `pom doctor` reports a refused or unauthenticated exposure.

Gaps and non-goals:

- "Local" means a loopback peer, or a peer whose address equals the listening address, that is not in
  `proxy.trusted_proxies` and asks by a `localhost` name (`localhost`, `*.localhost`, or a loopback IP). A
  request to the public domain, or one a trusted proxy forwards, always needs the token. List the reverse
  proxy in `proxy.trusted_proxies`: one that is not listed and rewrites `Host` to a loopback address still
  counts as local. Every local process needs no token, including other users of a shared host.
- The token cookie holds the raw token. Rotate with `pom proxy token new`.
- The in-memory request log (shown in the app) keeps request headers, but replaces the values of credential
  headers (`Authorization`, `Proxy-Authorization`, `Cookie`, `Set-Cookie`, and names containing `token`,
  `secret`, `password`, `api-key` or `session`) with `[redacted]` (`pom_proxy/src/pom_proxy.rs`,
  `sensitive_header`). Bodies are kept as sent. The log is not written to disk; the forwarded request is
  unchanged.

## Webhook relay

The relay (also `pom_proxy`, same bind as the proxy) takes `POST /<repo>/<service>/...` and fans the request
out to that service in every workspace where it is listening. It is meant to sit behind a public URL from a
tunnel (cloudflared, ngrok, a reverse proxy).

- Headers and body are forwarded unchanged, so a provider's own signature still
  verifies in your app. Verifying it is your app's job; the relay does not.
- Without a proxy token, anyone who learns a tunnel's public URL can post to the routed services. Keep tunnel
  URLs private and verify signatures in the app.
- With a token, a request that is not local (as for the proxy: a `localhost` name from this machine, not from
  a trusted proxy) must present it, as `?pom_token=` in the webhook URL or `X-Pom-Token`; a tunnel that keeps
  the public `Host` therefore needs it too. It is removed before fan-out (`relay_token`).
- Cross-site browser requests are refused with `403` on every relay path.
- Bodies are capped at 32 MiB and each delivery has a timeout.

`/_pom/github` is the one endpoint pom acts on itself: sync-on-push (`pom_push`). The delivery's
`X-Hub-Signature-256` is checked with HMAC-SHA256 against each project's webhook secret, compared in constant
time (`pom_push/src/signature.rs`); unsigned or wrongly signed deliveries are rejected. The payload only
triggers work: the worktree is fetched from its own `origin` and fast-forwarded, never set to a commit the
payload names. A replayed delivery causes at most another fetch. Setup steps that then run (dependency
installs, migrations) run code from `origin`, which you already trust.

## Agent control

`pom_agent` launches coding-agent sessions in PTY holders and lets people, orchestrators and other agents
drive them (`pom agent ...`, the MCP tools in `pom_mcp`).

- **Hooks.** Sessions are started with pom's hooks passed on the command line, and with the repo's own agent
  settings not loaded, so a repo cannot switch them off (`pom_agent/src/launch.rs`). Every event runs pom's
  hook command, which records state under `~/.local/state/pom/agents`.
- **PreToolUse policy.** A project can name a command under `agents.policy` in `pom.yml`. Before every tool
  call it answers allow, deny or ask. The check fails closed: an error, timeout, unreadable config or
  unknown answer denies the call (`pom_agent/src/policy.rs`). In a session nobody sits at, `ask` denies with
  `pending approval <id>` until that exact call is approved (`pom agent approve`).
- **PermissionRequest.** The agent's own permission prompts are recorded with an id. While pom holds the
  session, the hook waits for an answer from `pom agent allow|deny` or the `agent_approve` MCP tool; an id
  that no longer waits is refused as stale (`pom_agent/src/permission.rs`, `conversation.rs`). An agent may
  only deny another agent's prompt; an allow comes from a person or an orchestrator outside the agents,
  unless the project sets `agents.peer_approve` (`gate.rs`), which `config_set` cannot change.
- **Who may drive what.** `pom_agent/src/gate.rs` is the single place for these rules. The caller is
  classified by walking its process ancestry to an agent holder, not by environment variables
  (`caller.rs`). An agent may only reach sessions of its own workspace, with rate limits, a depth limit and
  a wait cap; people and orchestrators are not limited by default.
- **MCP server.** `pom mcp` is a stdio server started by the agent itself; it opens no port. Its tools act
  on the agent's own workspace. `secrets_list` returns names only. `run_in_env` runs commands and
  `config_set` rewrites `pom.yml`, which an agent with a shell could do anyway; `config_set` refuses any
  change to the `agents` section, and refuses to overwrite a `pom.yml` it cannot read, since the policy is
  read from it (`pom_mcp/src/config_files.rs`).
- **Screen fallback.** When the hook cannot carry an answer (a person took the session over, or the wait
  ran out), the agent draws its own permission dialog. pom never types an allow into it: an allow from
  `pom agent` or the MCP tools is refused with `allow_needs_hook`, and a person allows it at the terminal.
  A deny is the dialog's own Esc. pom types nothing while a person drives the session, reads the screen
  again right before every key and sends it only if the dialog is unchanged, and writes each key it sent or
  held back to the workspace log as a `screen_fallback` event (`pom_agent/src/fallback.rs`,
  `conversation.rs`). The folder-trust prompt at the start of a session pom launched is answered with yes
  under the same rules (`drive.rs`); `--trust` marks the folder trusted in the agent's config instead.

Trust boundary: these controls keep a well-behaved agent inside its workspace and keep mistakes from
spreading. They are not a sandbox. Everything runs as your user, and approvals, leases and policy answers
are files in your state folder, so a process that is determined to escape (for example by detaching from
its holder's process tree, or by writing those files directly) can. `config_set` will not touch the
`agents` section, but an agent with a shell can still edit `pom.yml` or the policy script directly, so policy
authors should deny those edits too. Run
agents you do not trust in a VM or container.

## Remote hosts (`pom server`)

A project on another machine is reached over ssh. The app runs `pom server proxy` on the host, which starts
`pom server` on demand: one process per user and version, listening on a Unix socket in
`$XDG_RUNTIME_DIR/pom-server` or `/tmp/pom-server-<uid>`. It never opens a network port; ports the app needs
(the host's dev proxy, shared services) are forwarded with `ssh -L` to this machine's loopback only
(`pom_remote/src/forward.rs`). The systemd unit is opt-in (`pom server install-unit`).

- The socket folder is created `0700`, and an existing one must be owned by the user or the server refuses
  to start (`pom_server/src/paths.rs`, `ensure_private_dir`). Every connection's peer uid must equal the
  server's (`peer.rs`, `server.rs`).
- ssh is the authentication. A client that reaches the socket is your own user and can open shells, so the
  server does not treat it as hostile; its checks stop bugs and stale clients from touching the wrong files.
- A project config path must be absolute, contain no `..`, and be a project registered on the host
  (`dispatch.rs`, `registered_config`). Branch names, op ids and holder names are validated.
- File access goes through `files::guard` (`files/src/guard.rs`): relative paths only, no `..`, NUL or
  backslash, resolved through symlinks to stay inside the workspace, regular files and folders only; `.git`
  and, in the main workspace, everything but the project config are read-only. A shell's working folder must
  resolve inside the worktree (`pom_server/src/pty.rs`, `check_cwd`).
- The Jira token is read on the host from the host's own settings and used there; it is not sent to the app
  (`pom_server/src/jira.rs`).

## Secrets

- `pom_secrets` keeps each session's secrets in `~/.local/state/pom/secrets/<session>.bin`: a random nonce
  and AES-256-GCM of the name/value map, under a per-machine key in `~/.local/state/pom/secret.key`. Both
  are written `0600`. The key sits next to the data, so encryption at rest protects a copied or backed-up
  file, not the data from a process of your own user. The key is created only when it is missing, never in
  place of one that cannot be read or has the wrong length: those are errors that leave the file as it is,
  so a backup can be put back.
- Onboarding imports a project's gitignored `.env*` values into this store. Config refers to them as
  `{{secret.NAME}}`; agents and the MCP server only ever see names.
- Values are injected into a service's environment at spawn, and also written into the generated env files
  (`.env.local` and the like) inside each worktree, as plain text, so apps that read a file find them
  (`pom_services/src/env_files.rs`). They are written `0600` through a temp file and rename, so a file left
  more open is replaced. Treat a worktree like a directory holding a `.env`.
- `pom ws export` (`.pomws`) and `pom project export` (`.pomproj`) carry only the secrets the config uses,
  decrypted and re-sealed with a passphrase (scrypt, then AES-256-GCM; `pom_bundle`). The machine key never
  travels. The archive's strength is the passphrase's, so use a long one and send it separately.
- Importing an archive applies its patches, untracked files, database dumps and volumes into a workspace:
  only import archives from people you trust. Archive entries must be plain files under the expected names,
  and every path the manifest names must be one of them; repo names are single folder names
  (`pom_transfer/src/pom_transfer.rs`, `allowed_entry`). The untracked-files tar unpacks only plain files
  and folders inside the worktree, never under a `.git`, with setuid bits dropped; a link or any other entry
  stops the import (`pom_workspace/src/import.rs`).
- Settings store env-var names, never secret values. The proxy token and Jira token fall back to the
  encrypted store when no env var is set.

## Updates

- **macOS app.** Releases are built, Developer ID signed and notarized in CI only. The in-app updater
  downloads the update tarball and installs it only if its Ed25519 signature verifies against the public key
  built into the app (`auto_update/src/releases.rs`, `verify`); the Sparkle `appcast.xml` is signed with
  the same key. The private key exists only as a CI secret and is never regenerated.
- **Linux.** Each tarball has a `.sig` made with the same key. The Linux app installed with `install.sh`
  and `pom self-update` install a tarball only after its signature verifies (`auto_update/src/linux_install.rs`);
  an install `install.sh` did not make (a package manager, a copy elsewhere) never replaces itself.
  `checksums-linux.txt` (sha256) stays next to the tarballs.
- **`pom` on a host.** "Update pom on host" downloads the host's tarball on this machine over https only,
  checks its signature against the key built into the app, then streams it over ssh to a temp folder on the
  host and runs its `install.sh --cli-only` (`pom_remote/src/update.rs`). The host downloads nothing itself.
  It installs the app's own version, or for a nightly app whose build has left the nightly release, the
  newest nightly.
- **Nightly builds** are built, signed and notarized by the same CI steps as a release and published to the
  `nightly` prerelease; only apps on the nightly channel install them.
- **Grammar packages** are wasm files whose signature is checked against a key built into the app (`grammars`).

App and host versions: the same version works fully. With different versions the app uses only what the
host's `pom server` declares in its HELLO, and offers "Update pom on host" once their major or minor version
differs.

## Local processes

- Every service, shell and agent runs in a PTY holder process behind a Unix socket in
  `$XDG_RUNTIME_DIR/pom-pty` or `/tmp/pom-pty-<uid>`. The folder is created `0700` and each socket is set
  to `0600` (`pom_ptyhost/src/server.rs`); anyone who can open a socket can type into that terminal. Before
  a holder listens or a client connects, the folder must be a real directory (not a symlink) owned by your
  user; a looser mode is tightened to `0700`, and a folder another user owns is refused, the same check
  `pom server` makes for its own socket folder (`pom_paths`, `ensure_private_dir`).
- ssh connection sharing uses control sockets in `$XDG_RUNTIME_DIR/pom-ssh` or `~/.ssh/pom`.
- Commands run as separate arguments, never as a shell string built from user input, and network calls have
  timeouts.

Gap: on a shared Linux host without `XDG_RUNTIME_DIR`, another user can still create `/tmp/pom-pty-<uid>`
first. Pomelo then refuses to start or reach terminals there instead of using it; remove the folder or set
`XDG_RUNTIME_DIR`.
