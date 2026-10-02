use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let home = std::env::temp_dir().join(format!(
            "ksl-session-isolation-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(home.join("kimi-statusline")).unwrap();
        std::fs::write(
            home.join("kimi-statusline/config.toml"),
            "[[segments]]\nid = 'tps'\n[[segments]]\nid = 'usage'\n[[segments]]\nid = 'subagent'\n[[segments]]\nid = 'session'\n",
        )
        .unwrap();
        let old = home.join("sessions/wd_demo/session_old");
        std::fs::create_dir_all(old.join("agents/main")).unwrap();
        std::fs::write(
            old.join("state.json"),
            r#"{"cwd":"/work/demo","createdAt":4102444800000}"#,
        )
        .unwrap();
        std::fs::write(
            old.join("agents/main/wire.jsonl"),
            concat!(
                "{\"type\":\"usage.record\",\"usage\":{\"inputOther\":100,\"output\":420}}\n",
                "{\"type\":\"context.append_loop_event\",\"event\":{\"type\":\"step.end\",\"usage\":{\"output\":420},\"llmStreamDurationMs\":10000},\"time\":100000}\n"
            ),
        )
        .unwrap();
        Self(home)
    }

    fn run(&self, args: &[&str], session_id: &str) -> String {
        // Most tests assert a completed refresh. Exercise cold rendering and
        // worker scheduling separately without timing-dependent polling.
        if !session_id.is_empty() && args.is_empty() {
            assert!(self
                .command(&["probe-session", "--session-id", session_id])
                .status()
                .unwrap()
                .success());
        }
        self.render(
            args,
            serde_json::json!({"sessionId": session_id, "cwd": "/work/demo"}),
        )
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_kimi-statusline"));
        command
            .args(args)
            .env("KIMI_CODE_HOME", &self.0)
            .env("KIMI_STATUSLINE_NO_COLOR", "1")
            .env_remove("KIMI_CODE_BASE_URL")
            .env_remove("KIMI_CODE_OAUTH_HOST")
            .env_remove("KIMI_OAUTH_HOST");
        let bin = self.0.join("bin");
        if bin.is_dir() {
            let mut paths = vec![bin];
            paths.extend(std::env::split_paths(
                &std::env::var_os("PATH").unwrap_or_default(),
            ));
            command.env("PATH", std::env::join_paths(paths).unwrap());
        }
        command
    }

    fn render(&self, args: &[&str], payload: serde_json::Value) -> String {
        let mut child = self
            .command(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        write!(child.stdin.take().unwrap(), "{}", payload).unwrap();
        let result = child.wait_with_output().unwrap();
        assert!(result.status.success(), "{:?}", result.stderr);
        String::from_utf8(result.stdout).unwrap().trim().to_string()
    }
}

#[test]
fn quota_isolated_on_logout_token_rotation_and_endpoint_changes() {
    use sha2::{Digest, Sha256};
    use std::io::{BufRead, BufReader};
    use std::time::{Duration, Instant};
    let f = Fixture::new();
    std::fs::write(
        f.0.join("kimi-statusline/config.toml"),
        "[[segments]]\nid='quota'\n",
    )
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}/coding/v1", listener.local_addr().unwrap());
    let (requested_tx, requested_rx) = std::sync::mpsc::channel();
    let (reply_tx, reply_rx) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let (mut stream, _) = loop {
            match listener.accept() {
                Ok(pair) => break pair,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    std::thread::yield_now()
                }
                Err(e) => panic!("quota request did not arrive: {e}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut headers = String::new();
        loop {
            let mut line = String::new();
            assert!(reader.read_line(&mut line).unwrap() > 0);
            if line == "\r\n" {
                break;
            }
            headers.push_str(&line);
        }
        assert!(headers.starts_with("GET /coding/v1/usages "));
        assert!(headers
            .to_lowercase()
            .contains("authorization: bearer account-a"));
        requested_tx.send(()).unwrap();
        reply_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let body = r#"{"usages":{"limit_5h":{"used_ratio":0.91}}}"#;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    });
    // Credential slots follow the upstream public endpoint hashing contract.
    let slot = |base: &str| {
        let input = format!(r#"{{"oauthHost":"https://auth.kimi.com","baseUrl":"{base}"}}"#);
        let hash = format!("{:x}", Sha256::digest(input.as_bytes()));
        format!("kimi-code-env-{}.json", &hash[..16])
    };
    let configure = |base: &str| {
        std::fs::write(
            f.0.join("config.toml"),
            format!("[providers.'managed:kimi-code']\nbase_url='{base}'\n"),
        )
        .unwrap()
    };
    let credentials = f.0.join("credentials");
    std::fs::create_dir_all(&credentials).unwrap();
    configure(&base);
    let credential = credentials.join(slot(&base));
    std::fs::write(
        &credential,
        r#"{"access_token":"account-a","expires_at":4102444800}"#,
    )
    .unwrap();
    let request = f
        .command(&["quota"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    requested_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    std::fs::write(
        &credential,
        r#"{"access_token":"account-b","expires_at":1}"#,
    )
    .unwrap();
    reply_tx.send(()).unwrap();
    let response = request.wait_with_output().unwrap();
    server.join().unwrap();
    assert!(response.status.success(), "{:?}", response.stderr);
    assert!(!f.command(&["quota"]).output().unwrap().status.success());
    assert_eq!(
        f.run(&[], ""),
        "",
        "in-flight response must remain scoped to account A"
    );
    std::fs::write(
        &credential,
        r#"{"access_token":"account-a","expires_at":4102444800}"#,
    )
    .unwrap();
    assert!(f.run(&[], "").contains("91%"));
    // Expiration alone retains quota for the same login.
    std::fs::write(
        &credential,
        r#"{"access_token":"account-a","expires_at":1}"#,
    )
    .unwrap();
    assert!(f.run(&[], "").contains("91%"));
    // A new token represents a potentially different account. An expired
    // fixture prevents network traffic while exercising failed refreshes.
    std::fs::write(
        &credential,
        r#"{"access_token":"account-b","expires_at":1}"#,
    )
    .unwrap();
    assert!(!f.command(&["quota"]).output().unwrap().status.success());
    assert_eq!(f.run(&[], ""), "");
    let other = "http://127.0.0.1:1/other";
    configure(other);
    std::fs::write(
        credentials.join(slot(other)),
        r#"{"access_token":"account-a","expires_at":1}"#,
    )
    .unwrap();
    assert!(!f.command(&["quota"]).output().unwrap().status.success());
    assert_eq!(f.run(&[], ""), "");
    configure(&base);
    std::fs::write(
        &credential,
        r#"{"access_token":"account-a","expires_at":1}"#,
    )
    .unwrap();
    assert!(f.run(&[], "").contains("91%"));
    std::fs::remove_file(&credential).unwrap();
    assert_eq!(f.run(&[], ""), "");
    std::fs::write(&credential, r#"{"access_token":"","expires_at":0}"#).unwrap();
    assert_eq!(f.run(&[], ""), "");
}

#[test]
fn disabled_session_segments_do_not_scan_or_cache_wires() {
    let f = Fixture::new();
    std::fs::write(
        f.0.join("kimi-statusline/config.toml"),
        "[[segments]]\nid='directory'\n",
    )
    .unwrap();
    assert!(!f
        .render(
            &[],
            serde_json::json!({"sessionId":"old","cwd":"/work/demo"})
        )
        .is_empty());
    assert!(!f.0.join("kimi-statusline-cache").exists());
}

#[test]
fn live_render_uses_published_snapshot_while_session_worker_is_locked() {
    let f = Fixture::new();
    let expected = f.run(&[], "old");
    let cache = f.0.join("kimi-statusline-cache");
    let snapshot = std::fs::read_dir(&cache)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("session-view-")
                && p.extension().is_some_and(|e| e == "json")
        })
        .unwrap();
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(snapshot.with_extension("lock"))
        .unwrap();
    lock.lock().unwrap();
    let mut stale: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&snapshot).unwrap()).unwrap();
    stale["t"] = 0.into();
    let before = serde_json::to_vec(&stale).unwrap();
    std::fs::write(&snapshot, &before).unwrap();
    // A live refresh must not resolve directories or parse changed wires.
    let sessions = f.0.join("sessions");
    let moved = f.0.join("temporarily-moved");
    std::fs::rename(&sessions, &moved).unwrap();
    for _ in 0..3 {
        assert_eq!(
            f.render(
                &[],
                serde_json::json!({"sessionId":"old","cwd":"/work/demo"})
            ),
            expected
        );
        assert_eq!(std::fs::read(&snapshot).unwrap(), before);
    }
    // An uncached session renders empty while its worker owns the same lock.
    std::fs::remove_file(&snapshot).unwrap();
    assert_eq!(f.render(&[], serde_json::json!({"sessionId":"old"})), "");
    assert!(!snapshot.exists());
    std::fs::rename(moved, sessions).unwrap();
    drop(lock);
    assert_eq!(f.run(&[], "old"), expected);
}

#[cfg(unix)]
#[test]
fn pr_worker_times_out_without_duplicate_processes_and_isolates_branches() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};
    let f = Fixture::new();
    let bin = f.0.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let gh = bin.join("gh");
    // The marker is written before exec, so the worker holds its lock when
    // foreground renders run. The sleep is the hanging command under test.
    std::fs::write(
        &gh,
        "#!/bin/sh\nprintf 'called\\n' >> calls\nexec sleep 30\n",
    )
    .unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(
        f.0.join("kimi-statusline/config.toml"),
        "[[segments]]\nid='git'\noptions={status=false,pr=true}\n",
    )
    .unwrap();
    let cwd = f.0.to_str().unwrap();
    let mut worker = f
        .command(&["probe-pr", "--cwd", cwd, "--branch", "alpha"])
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !f.0.join("calls").exists() && Instant::now() < deadline {
        std::thread::yield_now();
    }
    for _ in 0..3 {
        f.render(&[], serde_json::json!({"cwd":cwd,"gitBranch":"alpha"}));
    }
    assert!(worker.wait().unwrap().success());
    assert_eq!(
        std::fs::read_to_string(f.0.join("calls"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    assert!(!f
        .render(&[], serde_json::json!({"cwd":cwd,"gitBranch":"alpha"}))
        .contains("PR#"));
    std::fs::write(&gh, "#!/bin/sh\ncase \"$*\" in\n  *'-- alpha') printf '%s\\n' '{\"number\":11,\"url\":\"https://example.test/11\"}' ;;\n  *'-- beta') printf '%s\\n' '{\"number\":22,\"url\":\"https://example.test/22\"}' ;;\n  *) exit 2 ;;\nesac\n").unwrap();
    for branch in ["alpha", "beta"] {
        assert!(f
            .command(&["probe-pr", "--cwd", cwd, "--branch", branch])
            .status()
            .unwrap()
            .success());
    }
    assert!(f
        .render(&[], serde_json::json!({"cwd":cwd,"gitBranch":"alpha"}))
        .contains("PR#11"));
    assert!(f
        .render(&[], serde_json::json!({"cwd":cwd,"gitBranch":"beta"}))
        .contains("PR#22"));
}

#[test]
fn session_location_cache_recovers_after_a_move() {
    let f = Fixture::new();
    let original = f.run(&[], "old");
    let old = f.0.join("sessions/wd_demo/session_old");
    let moved = f.0.join("moved-session");
    std::fs::rename(&old, &moved).unwrap();
    let index = serde_json::json!({"sessionId":"old","sessionDir":moved});
    std::fs::write(f.0.join("session_index.jsonl"), format!("{index}\n")).unwrap();
    assert_eq!(f.run(&[], "old"), original);
}

#[test]
fn current_goal_events_override_legacy_metadata_across_refreshes() {
    let f = Fixture::new();
    std::fs::write(
        f.0.join("kimi-statusline/config.toml"),
        "[[segments]]\nid='goal'\n",
    )
    .unwrap();
    let dir = f.0.join("sessions/wd_demo/session_old");
    std::fs::write(
        dir.join("state.json"),
        r#"{"custom":{"goal":{"status":"active","turnsUsed":99}}}"#,
    )
    .unwrap();
    let wire = dir.join("agents/main/wire.jsonl");
    std::fs::write(&wire, concat!(
        "{\"type\":\"goal.create\",\"agentId\":\"main\",\"goalId\":\"g\",\"objective\":\"demo\",\"time\":1000}\n",
        "{\"type\":\"goal.update\",\"agentId\":\"main\",\"status\":\"paused\",\"turnsUsed\":3,\"wallClockMs\":120000,\"budgetLimits\":{\"turnBudget\":8},\"time\":121000}\n"
    )).unwrap();
    let paused = f.run(&[], "old");
    assert!(
        paused.contains("paused") && paused.contains("2m") && paused.contains("3/8 turns"),
        "{paused}"
    );
    assert_eq!(f.run(&[], "old"), paused);
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&wire)
        .unwrap();
    let resumed_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        - 60000;
    writeln!(file, r#"{{"type":"goal.update","agentId":"main","status":"active","wallClockResumedAt":{resumed_at}}}"#).unwrap();
    let active = f.run(&[], "old");
    assert!(
        active.contains("active") && active.contains("3m"),
        "{active}"
    );
    writeln!(
        file,
        r#"{{"type":"goal.update","agentId":"main","status":"complete"}}"#
    )
    .unwrap();
    assert_eq!(f.run(&[], "old"), "");
    writeln!(
        file,
        r#"{{"type":"goal.clear","agentId":"main","time":122000}}"#
    )
    .unwrap();
    assert_eq!(
        f.run(&[], "old"),
        "",
        "clear must not revive the legacy goal"
    );
    writeln!(
        file,
        r#"{{"type":"goal.create","agentId":"main","goalId":"next","objective":"demo"}}"#
    )
    .unwrap();
    assert!(f.run(&[], "old").contains("active"));
    writeln!(
        file,
        r#"{{"type":"forked","agentId":"main","fromAgentId":"main"}}"#
    )
    .unwrap();
    assert_eq!(f.run(&[], "old"), "", "fork clears inherited goal");
}

#[test]
fn tool_wait_is_not_counted_as_parallel_generation() {
    let f = Fixture::new();
    std::fs::write(
        f.0.join("kimi-statusline/config.toml"),
        "[[segments]]\nid='tps'\n",
    )
    .unwrap();
    let dir = f.0.join("sessions/wd_demo/session_old/agents");
    for (agent, stream_end) in [("main", 10000), ("child", 60000)] {
        let agent_dir = dir.join(agent);
        std::fs::create_dir_all(&agent_dir).unwrap();
        let usage = format!(
            r#"{{"type":"usage.record","agentId":"{agent}","usageScope":"turn","usage":{{"output":400}},"time":{stream_end}}}"#
        );
        let step = format!(
            r#"{{"type":"context.append_loop_event","agentId":"{agent}","event":{{"type":"step.end","usage":{{"output":400}},"llmStreamDurationMs":10000}},"time":60000}}"#
        );
        std::fs::write(agent_dir.join("wire.jsonl"), format!("{usage}\n{step}\n")).unwrap();
    }
    let line = f.run(&[], "old");
    assert!(line.contains("40.0 tok/s"), "{line}");
    assert!(
        !line.contains('×'),
        "serial generation reported as parallel: {line}"
    );
}

#[test]
fn installed_commands_work_with_upstream_node_spawn() {
    if Command::new("node").arg("--version").output().is_err() {
        // CI installs Node; locally the upstream spawn contract cannot be checked.
        assert!(std::env::var_os("CI").is_none(), "node is required in CI");
        eprintln!("skipping: node not found");
        return;
    }
    let names: &[&str] = if cfg!(windows) {
        // Without spaces, , ; = still split cmd's command token.
        &["safe", "space and&(test)", "comma,semi;eq=test"]
    } else {
        &["safe", "space'dollar$UNSET_KSL_TEST;and&paren(test)"]
    };
    for &name in names {
        let f = Fixture::new();
        let dir = f.0.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join(if cfg!(windows) {
            "kimi-statusline.exe"
        } else {
            "kimi-statusline"
        });
        std::fs::copy(env!("CARGO_BIN_EXE_kimi-statusline"), &exe).unwrap();
        let installed = Command::new(&exe)
            .arg("install")
            .env("KIMI_CODE_HOME", &f.0)
            .output()
            .unwrap();
        // CI runners keep 8.3 names on the temp volume, so the special paths
        // must really install and run there instead of taking the error branch.
        if cfg!(windows) && !installed.status.success() && std::env::var_os("CI").is_none() {
            // Filesystems can disable 8.3 names. Installation must fail clearly
            // and leave config untouched, never install a command cmd cannot run.
            assert!(
                String::from_utf8_lossy(&installed.stderr)
                    .contains("No shell-safe 8.3 path is available"),
                "{:?}",
                installed.stderr
            );
            assert!(!f.0.join("tui.toml").exists());
            continue;
        }
        assert!(installed.status.success(), "{:?}", installed.stderr);
        let doc: toml::Table = std::fs::read_to_string(f.0.join("tui.toml"))
            .unwrap()
            .parse()
            .unwrap();
        let command = doc["status_line"]["command"].as_str().unwrap();
        // Match the upstream Node spawn contract; Rust's cmd quoting differs.
        let output = Command::new("node")
            .args([
                "-e",
                r#"
        const {spawn} = require('node:child_process');
        const win = process.platform === 'win32';
        const command = process.argv[1] + ' --version';
        const child = spawn(win ? (process.env.ComSpec || 'cmd.exe') : 'sh',
            win ? ['/d', '/s', '/c', command] : ['-c', command],
            {stdio: ['ignore', 'pipe', 'pipe'], detached: !win});
        child.stdout.pipe(process.stdout);
        child.stderr.pipe(process.stderr);
        child.on('error', () => process.exit(1));
        child.on('close', code => process.exit(code ?? 1));
    "#,
                command,
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "{:?}", output.stderr);
        assert!(String::from_utf8_lossy(&output.stdout).starts_with("kimi-statusline "));
        assert!(Command::new(&exe)
            .arg("uninstall")
            .env("KIMI_CODE_HOME", &f.0)
            .output()
            .unwrap()
            .status
            .success());
        let after: toml::Table = std::fs::read_to_string(f.0.join("tui.toml"))
            .unwrap()
            .parse()
            .unwrap();
        assert!(
            after.get("status_line").is_none(),
            "short paths must retain command ownership"
        );
    }
}

#[test]
fn empty_kimi_home_uses_platform_home_directory() {
    let f = Fixture::new();
    let home = f.0.join("home");
    let profile = f.0.join("profile");
    let result = f
        .command(&["install", "--command", "kimi-statusline"])
        .env("KIMI_CODE_HOME", "")
        .env("HOME", &home)
        .env("USERPROFILE", &profile)
        .output()
        .unwrap();
    assert!(result.status.success(), "{:?}", result.stderr);
    let (selected, other) = if cfg!(windows) {
        (profile, home)
    } else {
        (home, profile)
    };
    assert!(selected.join(".kimi-code/tui.toml").is_file());
    assert!(!other.join(".kimi-code/tui.toml").exists());
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn live_sessions_do_not_inherit_previous_stats_but_preview_can_select_latest() {
    let f = Fixture::new();
    let old = f.run(&[], "old");
    assert!(old.contains("42.0 tok/s") && old.contains("420"), "{old}");
    assert_eq!(f.run(&[], "old"), old, "resuming uses the session's cache");

    assert_eq!(f.run(&[], "brand-new"), "", "not-yet-created session");
    assert_eq!(f.run(&[], ""), "", "startup before lazy session creation");
    let preview = ["preview", "--cwd", "/work/demo"];
    assert_eq!(
        f.run(&preview, ""),
        old,
        "preview explicitly selects latest"
    );
    assert_eq!(
        f.run(
            &["preview", "--cwd", "/work/demo", "--session", "missing"],
            ""
        ),
        "",
        "an explicit preview session must not select a different session"
    );

    // Releases before this fix could store another session's stats under a
    // new ID. Even that legacy cache must not populate a fresh session.
    let cache = f.0.join("kimi-statusline-cache");
    let old_cache = std::fs::read_dir(&cache)
        .unwrap()
        .filter_map(Result::ok)
        .find(|e| e.file_name().to_string_lossy().starts_with("session-"))
        .unwrap();
    // v0.4.0 named this cache using FNV-1a("brand-new").
    std::fs::copy(
        old_cache.path(),
        cache.join("session-75524ed797ecf1ed.json"),
    )
    .unwrap();
    std::fs::create_dir_all(f.0.join("sessions/wd_demo/session_brand-new/agents/main")).unwrap();
    assert_eq!(
        f.run(&[], "brand-new"),
        "",
        "fresh session with legacy cache"
    );
    std::fs::write(
        f.0.join("sessions/wd_demo/session_brand-new/agents/main/wire.jsonl"),
        concat!(
            "{\"type\":\"usage.record\",\"usage\":{\"inputOther\":50,\"output\":120}}\n",
            "{\"type\":\"context.append_loop_event\",\"event\":{\"type\":\"step.end\",\"usage\":{\"output\":120},\"llmStreamDurationMs\":2000},\"time\":200000}\n"
        ),
    )
    .unwrap();
    let fresh = f.run(&[], "brand-new");
    assert!(
        fresh.contains("60.0 tok/s") && fresh.contains("120"),
        "{fresh}"
    );
    assert!(!fresh.contains("42.0") && !fresh.contains("420"), "{fresh}");
    assert_eq!(
        f.run(&[], "old"),
        old,
        "original session is still resumable"
    );
}
