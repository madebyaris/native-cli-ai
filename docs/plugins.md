# Plugins

nca plugins are **WASM components**. They run in a wasmtime sandbox and can override
four areas of nca by calling back into the core implementation:

| Hook | What a plugin can do | Core behavior it can reuse |
|---|---|---|
| `auth` | Replace the API key / base URL of the default provider when it is built | `host.next-credential` (next plugin, then config/env) |
| `subagent` | Rewrite or deny a `spawn_subagent` request | `host.next-spawn-decision` |
| `agent` | Rewrite the system prompt; allow/deny each tool call | prompt is passed in; tool calls run after the built-in approval flow |
| `cosmetic` | Recolour TUI tokens | `host.next-theme` (the built-in palette) |

## Install

```bash
nca add https://github.com/acme/nca-plugin-sso          # public
nca add git@github.com:acme/private-plugin.git          # private: uses your own git credentials
nca add acme/nca-plugin-sso@v1.2.0                      # pin a tag, branch or commit
nca add <url> --project                                 # only for the current workspace
nca plugin list | info <name> | enable <name> | disable <name> | update [name] | remove <name>
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
`plugin`). A component must export all four hook interfaces; return `none` / `allow` for the
ones it does not care about. See `examples/plugin-template` (Rust, `wasm32-wasip2`):

```bash
rustup target add wasm32-wasip2
cd examples/plugin-template && ./build.sh      # -> plugin.wasm
nca add /path/to/plugin-template --yes         # local checkout works too
```

Chaining: plugins are ordered by `priority`, then name. A hook call enters the first plugin
that declared the hook; its `host.next-*` import runs the rest of the chain and finally core.
Returning `none` from `auth.resolve-credential` passes to the next plugin.

## Limits and failure policy

* Each guest call: 5 s wall clock (epoch interruption), 256 MiB memory.
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
