use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let home =
            std::env::temp_dir().join(format!("ksl-session-isolation-{}", std::process::id()));
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
        let mut child = Command::new(env!("CARGO_BIN_EXE_kimi-statusline"))
            .args(args)
            .env("KIMI_CODE_HOME", &self.0)
            .env("KIMI_STATUSLINE_NO_COLOR", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        write!(
            child.stdin.take().unwrap(),
            "{}",
            serde_json::json!({"sessionId": session_id, "cwd": "/work/demo"})
        )
        .unwrap();
        let result = child.wait_with_output().unwrap();
        assert!(result.status.success(), "{:?}", result.stderr);
        String::from_utf8(result.stdout).unwrap().trim().to_string()
    }
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
