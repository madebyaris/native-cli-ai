//! End-to-end: install the example plugin from a real git repo, then drive every
//! hook through the sandbox. Uses `examples/plugin-template/plugin.wasm`
//! (rebuild with `examples/plugin-template/build.sh` after changing the WIT).

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::host::PluginSet;
use crate::install::{self, InstallOptions};
use crate::registry::Registry;
use crate::types::{AuthRecovery, Credential, SpawnDecision, SpawnRequest, ToolDecision};

fn example_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugin-template")
}

fn git(cwd: &Path, args: &[&str]) {
    let st = Command::new("git")
        .args(["-c", "user.email=t@t", "-c", "user.name=t"])
        .args(args)
        .current_dir(cwd)
        .status()
        .unwrap();
    assert!(st.success(), "git {args:?}");
}

fn make_repo(root: &Path) -> PathBuf {
    let repo = root.join("plugin-repo");
    std::fs::create_dir_all(&repo).unwrap();
    for f in ["nca-plugin.toml", "plugin.wasm"] {
        std::fs::copy(example_dir().join(f), repo.join(f)).unwrap();
    }
    git(&repo, &["init", "--quiet"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "init"]);
    repo
}

#[test]
fn install_load_and_run_every_hook() {
    let tmp = tempfile::tempdir().unwrap();
    // SAFETY: this is the only test in the crate touching process env.
    unsafe {
        std::env::set_var("NCA_PLUGIN_HOME", tmp.path().join("home"));
        std::env::remove_var("EXAMPLE_API_KEY");
    }
    let repo = make_repo(tmp.path());
    let ws = tmp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    // Declined approval installs nothing.
    let deny = |_: &install::ApprovalRequest<'_>| false;
    let err = install::add(
        repo.to_str().unwrap(),
        &InstallOptions {
            workspace: None,
            approve: &deny,
        },
    )
    .unwrap_err();
    assert!(err.to_string().contains("not approved"), "{err}");
    assert!(Registry::load().unwrap().plugins.is_empty());

    // Approved install; approval sees the declared capabilities.
    let seen = std::cell::RefCell::new(Vec::new());
    let approve = |r: &install::ApprovalRequest<'_>| {
        seen.replace(r.manifest.capabilities.describe());
        true
    };
    let entry = install::add(
        repo.to_str().unwrap(),
        &InstallOptions {
            workspace: None,
            approve: &approve,
        },
    )
    .unwrap();
    assert_eq!(
        seen.borrow().as_slice(),
        [
            "write files: $NCA_PLUGIN_DATA",
            "environment: EXAMPLE_API_KEY"
        ]
    );
    assert_eq!(entry.name, "example");
    assert_eq!(entry.commit.len(), 40);

    // Duplicate install is refused.
    assert!(
        install::add(
            repo.to_str().unwrap(),
            &InstallOptions {
                workspace: None,
                approve: &approve
            }
        )
        .is_err()
    );

    let registry = Registry::load().unwrap();
    let set = PluginSet::load(&registry.plugins, &ws);
    assert_eq!(set.names(), ["example"]);

    // auth: no key in env -> falls through to the core default via next-credential.
    let core = Credential {
        api_key: Some("core-key".into()),
        base_url: Some("https://core".into()),
    };
    let got = set
        .resolve_credential("anthropic", Some(core.clone()))
        .unwrap();
    assert_eq!(got, Some(core.clone()));

    // auth: env var granted by the manifest -> plugin overrides.
    unsafe { std::env::set_var("EXAMPLE_API_KEY", "plugin-key") };
    let got = set
        .resolve_credential("anthropic", Some(core.clone()))
        .unwrap()
        .unwrap();
    assert_eq!(got.api_key.as_deref(), Some("plugin-key"));
    // other providers are untouched.
    assert_eq!(
        set.resolve_credential("openai", Some(core.clone()))
            .unwrap(),
        Some(core)
    );

    // subagent: plugin rewrites the request after consulting core.
    let req = SpawnRequest {
        task: "t".into(),
        focus_files: vec![],
        skills: vec![],
        use_worktree: false,
    };
    match set.before_spawn(req).unwrap() {
        SpawnDecision::Allow(r) => assert!(r.use_worktree),
        other => panic!("{other:?}"),
    }

    // agent: prompt rewrite + tool policy.
    let prompt = set.transform_system_prompt("base".into());
    assert!(prompt.starts_with("base\n\n") && prompt.contains("terse senior engineer"));
    assert_eq!(
        set.before_tool_use("execute_bash", r#"{"cmd":"ls"}"#),
        ToolDecision::Allow
    );
    match set.before_tool_use("execute_bash", r#"{"cmd":"rm -rf /"}"#) {
        ToolDecision::Deny(reason) => assert!(reason.contains("blocked"), "{reason}"),
        other => panic!("{other:?}"),
    }

    // cosmetic: plugin sees core defaults and overrides one token.
    let defaults = vec![
        ("assistant".to_string(), "#aaaaaa".to_string()),
        ("bg".to_string(), "#000000".to_string()),
    ];
    let theme = set.theme(defaults).unwrap();
    assert!(theme.contains(&("assistant".into(), "#f97316".into())));
    assert!(theme.contains(&("bg".into(), "#000000".into())));
    assert!(!theme.contains(&("assistant".into(), "#aaaaaa".into())));

    // login: sign in, use the saved token, refresh once on 401, then ask for re-login.
    unsafe { std::env::remove_var("EXAMPLE_API_KEY") };
    assert_eq!(
        set.login_plugins(),
        vec![("example".to_string(), vec!["anthropic".to_string()])]
    );
    let challenge = set.begin_login("anthropic").unwrap();
    assert_eq!(challenge.plugin, "example");
    assert!(challenge.paste_code && challenge.url.starts_with("https://"));
    assert!(
        set.begin_login("openai").is_err(),
        "no plugin signs in to openai"
    );
    let err = set.complete_login("anthropic", "bogus").unwrap_err();
    assert!(err.to_string().contains("not valid"), "{err}");
    // Before signing in there is nothing to refresh.
    assert!(matches!(
        set.on_auth_failure("anthropic", 401),
        AuthRecovery::LoginRequired(_)
    ));
    set.complete_login("anthropic", "code-abc").unwrap();
    let got = set.resolve_credential("anthropic", None).unwrap().unwrap();
    assert_eq!(got.api_key.as_deref(), Some("token-code-abc"));
    assert_eq!(
        set.on_auth_failure("anthropic", 401),
        AuthRecovery::Refreshed("token-refreshed-from-refresh-1".into())
    );
    let got = set.resolve_credential("anthropic", None).unwrap().unwrap();
    assert_eq!(
        got.api_key.as_deref(),
        Some("token-refreshed-from-refresh-1")
    );
    // The refresh token was spent: the next rejection means sign in again.
    assert!(matches!(
        set.on_auth_failure("anthropic", 403),
        AuthRecovery::LoginRequired(_)
    ));
    assert_eq!(set.on_auth_failure("openai", 401), AuthRecovery::NoPlugin);

    // Disabled and workspace-scoped plugins, checked on an unmodified install.
    let other = tmp.path().join("other-ws");
    std::fs::create_dir_all(&other).unwrap();
    install::set_enabled("example", None, false).unwrap();
    assert!(
        PluginSet::load(&Registry::load().unwrap().plugins, &ws).is_empty(),
        "a disabled plugin must not load"
    );
    install::set_enabled("example", None, true).unwrap();
    assert_eq!(
        PluginSet::load(&Registry::load().unwrap().plugins, &ws).names(),
        ["example"]
    );
    let project = install::add(
        repo.to_str().unwrap(),
        &InstallOptions {
            workspace: Some(other.clone()),
            approve: &approve,
        },
    )
    .unwrap();
    assert_eq!(project.workspace.as_deref(), other.to_str());
    let all = Registry::load().unwrap().plugins;
    // The project copy applies only to its own workspace, and shadows the global copy there.
    assert_eq!(PluginSet::load(&all, &ws).names(), ["example"]);
    assert_eq!(PluginSet::load(&all, &other).names(), ["example"]);
    install::set_enabled("example", None, false).unwrap();
    let all = Registry::load().unwrap().plugins;
    assert!(PluginSet::load(&all, &ws).is_empty());
    assert_eq!(PluginSet::load(&all, &other).names(), ["example"]);
    install::remove("example", Some(&other)).unwrap();
    install::set_enabled("example", None, true).unwrap();

    // Tampering with the installed wasm after approval is detected; plugin is skipped.
    let registry = Registry::load().unwrap();
    let wasm_path = entry.dir.join("plugin.wasm");
    let mut bytes = std::fs::read(&wasm_path).unwrap();
    bytes.push(0);
    std::fs::write(&wasm_path, bytes).unwrap();
    assert!(PluginSet::load(&registry.plugins, &ws).is_empty());
    assert!(
        registry.plugins[0]
            .load_verified()
            .unwrap_err()
            .to_string()
            .contains("hash mismatch")
    );

    // `remove` deletes the files.
    install::remove("example", None).unwrap();
    assert!(!entry.dir.exists());
    assert!(Registry::load().unwrap().plugins.is_empty());
}

#[test]
fn http_network_allow_by_default_but_deniable() {
    let err =
        crate::host::http_request_for_test(Some(&[]), "GET", "https://evil.example/x").unwrap_err();
    assert!(err.contains("not granted"), "{err}");
    let err = crate::host::http_request_for_test(None, "GET", "file:///etc/passwd").unwrap_err();
    assert!(err.contains("scheme"), "{err}");
    let err = crate::host::http_request_for_test(
        Some(&["a.example".to_string()]),
        "GET",
        "https://b.example/x",
    )
    .unwrap_err();
    assert!(err.contains("not granted"), "{err}");
}
