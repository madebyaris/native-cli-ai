# Plugins

nca plugins are **WASM components**. They run in a wasmtime sandbox and can override
four areas of nca by calling back into the core implementation:

| Hook | What a plugin can do | Core behavior it can reuse |
|---|---|---|
| `auth` | Replace the API key / base URL of the default provider when it is built | `host.next-credential` (next plugin, then config/env) |
| `subagent` | Rewrite or deny a `spawn_subagent` request | `host.next-spawn-decision` |
| `agent` | Rewrite the system prompt; allow/deny each tool call | prompt is passed in; tool calls run after the built-in approval flow |
| `cosmetic` | Recolour TUI tokens | `host.next-theme` (the built-in palette) |
| `login` (optional) | Interactive sign-in, refresh on 401/403, re-login | none: used only by `/login` and the provider's auth-failure path |

## Install

```bash
nca add https://github.com/acme/nca-plugin-sso          # public
nca add git@github.com:acme/private-plugin.git          # private: uses your own git credentials
nca add acme/nca-plugin-sso@v1.2.0                      # pin a tag, branch or commit
nca add <url> --project                                 # only for the current workspace
nca plugin list                                         # also: info <name>, enable <name>, disable <name>
nca plugin update [name]                                # re-fetch; asks again if grants changed
nca plugin remove <name>
```

`nca add` clones the repo, reads `nca-plugin.toml`, loads the prebuilt `.wasm` it names
(**nothing is built or executed from the repo**), shows hooks and capabilities, and asks
for approval (`--yes` to skip; required when not on a TTY). Only the manifest and the wasm
are installed, to `~/.nca/plugins/<name>/` (project scope: `<workspace>/.nca/plugins/<name>/`).

Trust model:

* `~/.nca/plugins/registry.json` records commit, wasm sha256 and manifest sha256 at approval
  time. A file that changed afterwards is **not loaded**. A repository cannot enable plugins
  by committing `.nca/plugins/`; only plugins in your home registry run.
* `nca plugin update` asks again only if hooks, capabilities or config changed.
* The host does the git clone. Credentials are never passed to plugins.

## Manifest (`nca-plugin.toml`, repository root)

```toml
[plugin]
name = "acme-sso"            # [a-z0-9-_]
version = "0.1.0"
api_version = "1"            # must match this nca
wasm = "plugin.wasm"         # prebuilt, relative to the repo root
priority = 0                 # optional; lower runs first

[hooks]                      # a hook not listed here is never called
auth = true
cosmetic = true

[capabilities]               # files and env: deny by default
# network: omit = any http(s) host (default); list = only those hosts; [] = none
network = ["login.acme.com"]                # bare hostnames, no redirects followed
fs_read = []                                # absolute paths, $HOME, $NCA_PLUGIN_DATA, $WORKSPACE
fs_write = ["$NCA_PLUGIN_DATA"]
env = ["ACME_TOKEN"]

[config]                     # readable via host.config-get
tenant = "acme"
```

## Writing a plugin

The interface is `crates/plugin/wit/nca-plugin.wit` (package `nca:plugin@1.0.0`, world
`plugin`). A component must export all four hook interfaces (a hook the manifest does not
declare is never called). What an unused hook should return:

| Hook | Defer to the rest of the chain / core | Notes |
| --- | --- | --- |
| `auth.resolve-credential` | `none`, or `host.next-credential` | an `err` fails closed |
| `subagent.before-spawn` | `host.next-spawn-decision` | must return `allow(request)` or `deny(reason)`; the first plugin that declares the hook decides, so it must call `next-spawn-decision` to let later plugins and core decide |
| `agent.transform-system-prompt` | `none` (keep the prompt) | every agent plugin runs, in order |
| `agent.before-tool-use` | `allow` | every agent plugin runs, in order; the first `deny` wins. The shell tool is named `execute_bash` |
| `cosmetic.resolve-theme` | `host.next-theme` | returns a theme; a partial one is merged over the defaults |

A plugin that also needs sign-in targets the world `plugin-login`
(the same plus `login`) and sets `login = true` under `[hooks]`; plugins built against `plugin`
keep working unchanged. See `examples/plugin-template` (Rust, `wasm32-wasip2`):

```bash
rustup target add wasm32-wasip2
cd examples/plugin-template && ./build.sh      # -> plugin.wasm
nca add /path/to/plugin-template --yes         # local checkout works too
```

Chaining: plugins are ordered by `priority`, then name. For `auth`, `subagent` and `cosmetic`, a
hook call enters the first plugin that declared the hook; its `host.next-*` import runs the rest of
the chain and finally core. Returning `none` from `auth.resolve-credential` passes to the next plugin.
The `agent` hooks have no `next` import: each declaring plugin is called in order.
If a project-scoped and a global plugin share a name, only the project one runs.

Scope: plugins load once per process, for the workspace nca was started in. Project-scoped plugins
apply to that workspace only; a long-lived host such as ACP that opens other workspaces keeps the
plugins of the startup directory.

## Sign-in and re-login (`login` hook)

For plugins that own a credential, such as an OAuth subscription token. The interface
(`nca:plugin/login`) has four functions:

| Function | Called when | Returns |
| --- | --- | --- |
| `providers` | nca needs to know who can sign in | provider ids, e.g. `anthropic` |
| `begin-login(provider)` | the user runs `/login` | `{ url, instructions, mode }`; nca prints the URL and tries to open the browser |
| `complete-login(provider, input)` | the user runs `/login <code>` | `ok`, or an error shown to the user |
| `on-auth-failure(provider, status)` | the provider answered 401/403 to a credential from this plugin | `some(key)`: retry once with it; `none`: sign in again; `err`: could not try |

Rules:

* Keep PKCE state and tokens in the plugin's own data directory: `fs_write = ["$NCA_PLUGIN_DATA"]`,
  and the plugin sees that path in the `NCA_PLUGIN_DATA` environment variable.
* Put endpoints and client ids in `[config]` and read them with `host.config-get`, so a change on
  the provider's side needs no rebuild.
* Sign-in calls get 40 s instead of 5 s, because they talk to the network.
* A dropped connection is not an auth failure. Make `on-auth-failure` return `err` when the
  refresh request itself fails (nca then reports "could not refresh", and the user stays signed in).
  Return `none` only when the server refused the refresh token. After `none`, or when the
  refreshed credential is rejected as well, nca stops the turn with `signed out: ...` and the TUI
  says to run `/login`. The session is kept.
* `examples/plugin-template` is a runnable skeleton of all four functions.

## Limits and failure policy

* Each guest call: 5 s wall clock (40 s for `login` calls) (epoch interruption), 256 MiB memory.
* stdout/stderr are discarded; use `host.log`. No sockets: HTTP goes through
  `host.http-request`, any http(s) host unless `capabilities.network` lists hosts (or is `[]`), 8 MiB response cap, no redirects.
* Filesystem and env access only as granted by the manifest.
* A plugin that fails to verify, compile or instantiate is skipped with a warning.
* Failures at call time: `auth` fails closed (provider is not built), `subagent` and
  `agent.before-tool-use` deny, `agent.transform-system-prompt` and `cosmetic` fall back to
  core (fail-open).
* A trap poisons that plugin instance for the rest of the process.

## Theme tokens

`bg surface border mention-bg user assistant tool muted text success error warn`, as
`#rrggbb`. Applied by remapping the rendered frame, so every widget using the palette follows.
