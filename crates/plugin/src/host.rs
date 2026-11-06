//! Sandboxed wasmtime host for nca plugins.
//!
//! Plugins form a chain ordered by `plugin.priority`. Each hook call enters the
//! first plugin that declared that hook; the plugin's `host.next-*` import runs
//! the rest of the chain and finally the built-in core behavior.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use wasmtime::component::{Component, HasSelf, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi::{FsPerms, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

use crate::error::PluginError;
use crate::manifest::{Capabilities, Hooks, Manifest};
use crate::registry::Entry;
use crate::types::{Credential, SpawnDecision, SpawnRequest, ThemeEntries, ToolDecision};

mod bindings {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "plugin",
    });
}
use bindings::Plugin as Bindings;
use bindings::PluginPre;
use bindings::nca::plugin::{host as wit_host, types as wit};

/// Wall-clock budget for one guest call (epoch interruption).
const CALL_TIMEOUT: Duration = Duration::from_secs(5);
const EPOCH_TICK: Duration = Duration::from_millis(50);
const MAX_MEMORY_BYTES: usize = 256 * 1024 * 1024;
const MAX_HTTP_BODY: u64 = 8 * 1024 * 1024;
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

fn engine() -> Result<&'static Engine, PluginError> {
    static ENGINE: OnceLock<Result<Engine, String>> = OnceLock::new();
    ENGINE
        .get_or_init(|| {
            let mut config = Config::new();
            config.wasm_component_model(true);
            config.epoch_interruption(true);
            let engine = Engine::new(&config).map_err(|e| e.to_string())?;
            let ticker = engine.clone();
            std::thread::Builder::new()
                .name("nca-plugin-epoch".into())
                .spawn(move || {
                    loop {
                        std::thread::sleep(EPOCH_TICK);
                        ticker.increment_epoch();
                    }
                })
                .map_err(|e| e.to_string())?;
            Ok(engine)
        })
        .as_ref()
        .map_err(|e| PluginError::Install(format!("wasm engine: {e}")))
}

fn new_linker() -> Result<Linker<HostState>, PluginError> {
    let mut linker = Linker::new(engine()?);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker).map_err(|e| rt_err("wasi", e))?;
    Bindings::add_to_linker::<HostState, HasSelf<HostState>>(&mut linker, |s| s)
        .map_err(|e| rt_err("host", e))?;
    Ok(linker)
}

fn rt_err(plugin: &str, e: impl std::fmt::Display) -> PluginError {
    PluginError::Runtime {
        plugin: plugin.to_string(),
        message: format!("{e:#}"),
    }
}

/// Verify the bytes are a component whose imports nca can satisfy.
pub fn check_component(wasm: &[u8]) -> Result<(), PluginError> {
    let engine = engine()?;
    let component = Component::new(engine, wasm)
        .map_err(|e| PluginError::Install(format!("not a valid WASM component: {e:#}")))?;
    let linker = new_linker()?;
    let pre = linker.instantiate_pre(&component).map_err(|e| {
        PluginError::Install(format!("plugin imports unsupported by this nca: {e:#}"))
    })?;
    PluginPre::new(pre).map_err(|e| {
        PluginError::Install(format!(
            "component does not implement the `nca:plugin/plugin@1` world: {e:#}"
        ))
    })?;
    Ok(())
}

// ── per-call defaults + store state ──────────────────────────────────────────

/// Core behavior for the current call, handed to the end of the chain.
#[derive(Clone, Default)]
pub(crate) struct Defaults {
    pub credential: Option<Credential>,
    pub theme: ThemeEntries,
}

struct HostState {
    wasi: WasiCtx,
    table: ResourceTable,
    limits: StoreLimits,
    name: String,
    config: BTreeMap<String, String>,
    allowed_env: Vec<String>,
    allowed_hosts: Vec<String>,
    next: Option<Arc<Node>>,
    defaults: Defaults,
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl wit::Host for HostState {}

impl wit_host::Host for HostState {
    fn log(&mut self, level: wit::LogLevel, message: String) {
        let plugin = self.name.as_str();
        match level {
            wit::LogLevel::Debug => tracing::debug!(plugin, "{message}"),
            wit::LogLevel::Info => tracing::info!(plugin, "{message}"),
            wit::LogLevel::Warn => tracing::warn!(plugin, "{message}"),
            wit::LogLevel::Error => tracing::error!(plugin, "{message}"),
        }
    }

    fn config_get(&mut self, key: String) -> Option<String> {
        self.config.get(&key).cloned()
    }

    fn env_get(&mut self, name: String) -> Option<String> {
        if self.allowed_env.contains(&name) {
            std::env::var(&name).ok()
        } else {
            tracing::warn!(plugin = %self.name, "denied env read of `{name}`");
            None
        }
    }

    fn http_request(
        &mut self,
        method: String,
        url: String,
        headers: Vec<(String, String)>,
        body: Option<Vec<u8>>,
    ) -> Result<wit_host::HttpResponse, String> {
        http_request(&self.allowed_hosts, &method, &url, headers, body)
    }

    fn next_credential(&mut self, provider: String) -> Result<Option<wit::Credential>, String> {
        let next = self.next.clone();
        chain_credential(next.as_ref(), &provider, &self.defaults)
            .map(|c| c.map(Into::into))
            .map_err(|e| e.to_string())
    }

    fn next_spawn_decision(
        &mut self,
        request: wit::SpawnRequest,
    ) -> Result<wit::SpawnDecision, String> {
        let next = self.next.clone();
        chain_spawn(next.as_ref(), request.into(), &self.defaults)
            .map(Into::into)
            .map_err(|e| e.to_string())
    }

    fn next_theme(&mut self) -> Result<wit::Theme, String> {
        let next = self.next.clone();
        chain_theme(next.as_ref(), &self.defaults).map_err(|e| e.to_string())
    }
}

fn http_request(
    allowed: &[String],
    method: &str,
    url: &str,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
) -> Result<wit_host::HttpResponse, String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("bad url: {e}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(format!("scheme `{}` is not allowed", parsed.scheme()));
    }
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    if !allowed.iter().any(|h| h.eq_ignore_ascii_case(&host)) {
        return Err(format!(
            "network access to `{host}` is not granted by the plugin manifest"
        ));
    }
    let method = reqwest::Method::from_bytes(method.as_bytes()).map_err(|e| e.to_string())?;
    // reqwest::blocking panics when started inside a tokio runtime thread, and
    // plugin hooks can be called from one, so run it on a scoped thread.
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                // Redirects are disabled: a 302 must not escape the allowlist.
                let client = reqwest::blocking::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .timeout(HTTP_TIMEOUT)
                    .build()
                    .map_err(|e| e.to_string())?;
                let mut req = client.request(method, parsed);
                for (k, v) in headers {
                    req = req.header(k, v);
                }
                if let Some(body) = body {
                    req = req.body(body);
                }
                let resp = req.send().map_err(|e| e.to_string())?;
                let status = resp.status().as_u16();
                let mut buf = Vec::new();
                resp.take(MAX_HTTP_BODY + 1)
                    .read_to_end(&mut buf)
                    .map_err(|e| e.to_string())?;
                if buf.len() as u64 > MAX_HTTP_BODY {
                    return Err(format!("response larger than {MAX_HTTP_BODY} bytes"));
                }
                Ok(wit_host::HttpResponse { status, body: buf })
            })
            .join()
            .unwrap_or_else(|_| Err("http worker panicked".into()))
    })
}

// ── nodes and chain execution ────────────────────────────────────────────────

struct Instance {
    store: Store<HostState>,
    bindings: Bindings,
}

pub(crate) struct Node {
    pub name: String,
    pub hooks: Hooks,
    next: Option<Arc<Node>>,
    instance: Mutex<Instance>,
}

impl Node {
    fn call<R: Send>(
        &self,
        defaults: &Defaults,
        f: impl FnOnce(&mut Bindings, &mut Store<HostState>) -> wasmtime::Result<R> + Send,
    ) -> Result<R, PluginError> {
        let mut guard = self
            .instance
            .lock()
            .map_err(|_| rt_err(&self.name, "plugin instance poisoned by an earlier trap"))?;
        let Instance { store, bindings } = &mut *guard;
        store.data_mut().defaults = defaults.clone();
        store.set_epoch_deadline((CALL_TIMEOUT.as_millis() / EPOCH_TICK.as_millis()) as u64);
        // wasmtime-wasi blocks on its own runtime for filesystem access, which panics if
        // the current thread is already driving a tokio runtime. Run the guest on a
        // plain scoped thread in that case.
        let out = if tokio::runtime::Handle::try_current().is_ok() {
            std::thread::scope(|scope| {
                scope
                    .spawn(|| f(bindings, store))
                    .join()
                    .unwrap_or_else(|_| Err(wasmtime::Error::msg("plugin call panicked")))
            })
        } else {
            f(bindings, store)
        };
        out.map_err(|e| rt_err(&self.name, e))
    }
}

type Link<'a> = Option<&'a Arc<Node>>;

fn skip_to<'a>(mut node: Link<'a>, wants: impl Fn(&Hooks) -> bool) -> Link<'a> {
    // Walk to the first node that declared the hook. Returned references point
    // into Arcs owned by the chain, which outlives the call.
    while let Some(n) = node {
        if wants(&n.hooks) {
            return Some(n);
        }
        node = n.next.as_ref();
    }
    None
}

pub(crate) fn chain_credential(
    node: Link<'_>,
    provider: &str,
    defaults: &Defaults,
) -> Result<Option<Credential>, PluginError> {
    let Some(node) = skip_to(node, |h| h.auth) else {
        return Ok(defaults.credential.clone());
    };
    let out = node.call(defaults, |b, s| {
        b.nca_plugin_auth().call_resolve_credential(s, provider)
    })?;
    match out {
        Ok(Some(cred)) => Ok(Some(cred.into())),
        // `none` = no opinion: whatever the rest of the chain / core says.
        Ok(None) => chain_credential(node.next.as_ref(), provider, defaults),
        Err(message) => Err(PluginError::Runtime {
            plugin: node.name.clone(),
            message,
        }),
    }
}

pub(crate) fn chain_spawn(
    node: Link<'_>,
    request: SpawnRequest,
    defaults: &Defaults,
) -> Result<SpawnDecision, PluginError> {
    let Some(node) = skip_to(node, |h| h.subagent) else {
        return Ok(SpawnDecision::Allow(request));
    };
    let out = node.call(defaults, |b, s| {
        b.nca_plugin_subagent()
            .call_before_spawn(s, &request.clone().into())
    })?;
    out.map(Into::into).map_err(|message| PluginError::Runtime {
        plugin: node.name.clone(),
        message,
    })
}

pub(crate) fn chain_theme(
    node: Link<'_>,
    defaults: &Defaults,
) -> Result<ThemeEntries, PluginError> {
    let Some(node) = skip_to(node, |h| h.cosmetic) else {
        return Ok(defaults.theme.clone());
    };
    let out = node.call(defaults, |b, s| {
        b.nca_plugin_cosmetic().call_resolve_theme(s)
    })?;
    out.map_err(|message| PluginError::Runtime {
        plugin: node.name.clone(),
        message,
    })
}

// ── PluginSet ────────────────────────────────────────────────────────────────

/// Loaded, instantiated plugins for one nca process.
pub struct PluginSet {
    head: Option<Arc<Node>>,
    names: Vec<String>,
    /// Plugins in chain order, for sequential (non-`next`) hooks.
    order: Vec<Arc<Node>>,
}

impl PluginSet {
    pub fn empty() -> Self {
        Self {
            head: None,
            names: Vec::new(),
            order: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// Load every enabled plugin that applies to `workspace`. A plugin that
    /// fails verification or instantiation is skipped with a warning; one bad
    /// plugin must not stop nca from starting.
    pub fn load(entries: &[Entry], workspace: &Path) -> Self {
        let mut loaded: Vec<(i32, String, Entry, Manifest, Vec<u8>)> = Vec::new();
        for entry in entries
            .iter()
            .filter(|e| e.enabled && e.applies_to(workspace))
        {
            match entry.load_verified() {
                Ok((manifest, wasm)) => loaded.push((
                    manifest.plugin.priority,
                    manifest.plugin.name.clone(),
                    entry.clone(),
                    manifest,
                    wasm,
                )),
                Err(e) => tracing::warn!("skipping plugin `{}`: {e}", entry.name),
            }
        }
        loaded.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));

        // Build tail-first so each node can own its `next`.
        let mut next: Option<Arc<Node>> = None;
        let mut order_rev = Vec::new();
        for (_, name, entry, manifest, wasm) in loaded.into_iter().rev() {
            match instantiate(&entry, &manifest, &wasm, workspace, next.clone()) {
                Ok(node) => {
                    let node = Arc::new(node);
                    order_rev.push(node.clone());
                    next = Some(node);
                }
                Err(e) => tracing::warn!("skipping plugin `{name}`: {e}"),
            }
        }
        order_rev.reverse();
        Self {
            head: next,
            names: order_rev.iter().map(|n| n.name.clone()).collect(),
            order: order_rev,
        }
    }

    pub(crate) fn resolve_credential(
        &self,
        provider: &str,
        default: Option<Credential>,
    ) -> Result<Option<Credential>, PluginError> {
        chain_credential(
            self.head.as_ref(),
            provider,
            &Defaults {
                credential: default,
                theme: Vec::new(),
            },
        )
    }

    pub(crate) fn before_spawn(&self, request: SpawnRequest) -> Result<SpawnDecision, PluginError> {
        chain_spawn(self.head.as_ref(), request, &Defaults::default())
    }

    pub(crate) fn theme(&self, default: ThemeEntries) -> Result<ThemeEntries, PluginError> {
        chain_theme(
            self.head.as_ref(),
            &Defaults {
                credential: None,
                theme: default,
            },
        )
    }

    /// Each agent plugin transforms the prompt in chain order.
    pub(crate) fn transform_system_prompt(&self, mut prompt: String) -> String {
        for node in self.order.iter().filter(|n| n.hooks.agent) {
            let current = prompt.clone();
            match node.call(&Defaults::default(), |b, s| {
                b.nca_plugin_agent()
                    .call_transform_system_prompt(s, &current)
            }) {
                Ok(Some(new)) => prompt = new,
                Ok(None) => {}
                Err(e) => tracing::warn!("{e}; keeping system prompt"),
            }
        }
        prompt
    }

    /// First deny wins. A failing plugin denies the call (fail-closed): this is
    /// a policy hook, so a crash must not silently disable the policy.
    pub(crate) fn before_tool_use(&self, tool: &str, input_json: &str) -> ToolDecision {
        for node in self.order.iter().filter(|n| n.hooks.agent) {
            match node.call(&Defaults::default(), |b, s| {
                b.nca_plugin_agent()
                    .call_before_tool_use(s, tool, input_json)
            }) {
                Ok(wit::ToolDecision::Allow) => {}
                Ok(wit::ToolDecision::Deny(reason)) => {
                    return ToolDecision::Deny(format!("plugin `{}`: {reason}", node.name));
                }
                Err(e) => return ToolDecision::Deny(e.to_string()),
            }
        }
        ToolDecision::Allow
    }
}

fn expand_path(raw: &str, data_dir: &Path, workspace: &Path) -> Result<PathBuf, PluginError> {
    let home = std::env::var("HOME").unwrap_or_default();
    let expanded = raw
        .replace("$HOME", &home)
        .replace("$NCA_PLUGIN_DATA", &data_dir.to_string_lossy())
        .replace("$WORKSPACE", &workspace.to_string_lossy());
    let path = PathBuf::from(expanded);
    if !path.is_absolute() {
        return Err(PluginError::Manifest(format!(
            "capability path `{raw}` must be absolute or start with $HOME / $NCA_PLUGIN_DATA / $WORKSPACE"
        )));
    }
    Ok(path)
}

fn build_wasi(
    caps: &Capabilities,
    data_dir: &Path,
    workspace: &Path,
    name: &str,
) -> Result<WasiCtx, PluginError> {
    // Stdio is not inherited (null streams); plugins log through `host.log`.
    let mut builder = WasiCtxBuilder::new();
    std::fs::create_dir_all(data_dir).map_err(|e| rt_err(name, e))?;
    for (paths, perms) in [
        (&caps.fs_read, FsPerms::ReadOnly),
        (&caps.fs_write, FsPerms::ReadWrite),
    ] {
        for raw in paths {
            let path = expand_path(raw, data_dir, workspace)?;
            let guest = path.to_string_lossy().into_owned();
            builder
                .preopened_dir(&path, guest, perms)
                .map_err(|e| rt_err(name, format!("cannot open `{}`: {e}", path.display())))?;
        }
    }
    Ok(builder.build())
}

fn instantiate(
    entry: &Entry,
    manifest: &Manifest,
    wasm: &[u8],
    workspace: &Path,
    next: Option<Arc<Node>>,
) -> Result<Node, PluginError> {
    let name = manifest.plugin.name.clone();
    let engine = engine()?;
    let component = Component::new(engine, wasm).map_err(|e| rt_err(&name, e))?;
    let linker = new_linker()?;
    let pre = PluginPre::new(
        linker
            .instantiate_pre(&component)
            .map_err(|e| rt_err(&name, e))?,
    )
    .map_err(|e| rt_err(&name, e))?;

    let state = HostState {
        wasi: build_wasi(&manifest.capabilities, &entry.data_dir(), workspace, &name)?,
        table: ResourceTable::new(),
        limits: StoreLimitsBuilder::new()
            .memory_size(MAX_MEMORY_BYTES)
            .instances(16)
            .build(),
        name: name.clone(),
        config: manifest.config.clone(),
        allowed_env: manifest.capabilities.env.clone(),
        allowed_hosts: manifest.capabilities.network.clone(),
        next: next.clone(),
        defaults: Defaults::default(),
    };
    let mut store = Store::new(engine, state);
    store.limiter(|s| &mut s.limits);
    store.set_epoch_deadline((CALL_TIMEOUT.as_millis() / EPOCH_TICK.as_millis()) as u64);
    let bindings = pre.instantiate(&mut store).map_err(|e| rt_err(&name, e))?;
    Ok(Node {
        name,
        hooks: manifest.hooks.clone(),
        next,
        instance: Mutex::new(Instance { store, bindings }),
    })
}

// ── conversions between public and generated types ──────────────────────────

impl From<wit::Credential> for Credential {
    fn from(c: wit::Credential) -> Self {
        Credential {
            api_key: c.api_key,
            base_url: c.base_url,
        }
    }
}
impl From<Credential> for wit::Credential {
    fn from(c: Credential) -> Self {
        wit::Credential {
            api_key: c.api_key,
            base_url: c.base_url,
        }
    }
}
impl From<wit::SpawnRequest> for SpawnRequest {
    fn from(r: wit::SpawnRequest) -> Self {
        SpawnRequest {
            task: r.task,
            focus_files: r.focus_files,
            skills: r.skills,
            use_worktree: r.use_worktree,
        }
    }
}
impl From<SpawnRequest> for wit::SpawnRequest {
    fn from(r: SpawnRequest) -> Self {
        wit::SpawnRequest {
            task: r.task,
            focus_files: r.focus_files,
            skills: r.skills,
            use_worktree: r.use_worktree,
        }
    }
}
impl From<wit::SpawnDecision> for SpawnDecision {
    fn from(d: wit::SpawnDecision) -> Self {
        match d {
            wit::SpawnDecision::Allow(r) => SpawnDecision::Allow(r.into()),
            wit::SpawnDecision::Deny(reason) => SpawnDecision::Deny(reason),
        }
    }
}
impl From<SpawnDecision> for wit::SpawnDecision {
    fn from(d: SpawnDecision) -> Self {
        match d {
            SpawnDecision::Allow(r) => wit::SpawnDecision::Allow(r.into()),
            SpawnDecision::Deny(reason) => wit::SpawnDecision::Deny(reason),
        }
    }
}

#[cfg(test)]
pub(crate) fn http_request_for_test(
    allowed: &[String],
    method: &str,
    url: &str,
) -> Result<(), String> {
    http_request(allowed, method, url, Vec::new(), None).map(|_| ())
}
