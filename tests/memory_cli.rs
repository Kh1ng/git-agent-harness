//! The native retrieval path must work with file-only credentials and preserve project scope.
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    process::Command,
    thread,
};

#[test]
fn project_memory_cli_uses_hook_url_and_private_key() {
    for role in ["central", "worker"] {
        check_file_credentials(role);
    }
}

fn check_file_credentials(role: &str) {
    let tmp = tempfile::tempdir().unwrap();
    let config_dir = tmp.path().join(".config/gah");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("tdai-gateway.env"),
        "TDAI_GATEWAY_API_KEY='test-file-key'\n",
    )
    .unwrap();
    fs::write(
        config_dir.join("gah-loop.env"),
        "COORDINATOR_TOKEN=\"test-coordinator-key\"\n",
    )
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    fs::write(
        config_dir.join("memory-hooks.json"),
        serde_json::json!({"gateway_url":url}).to_string(),
    )
    .unwrap();
    let config = tmp.path().join("config.toml");
    fs::write(
        &config,
        format!(
            r#"
[defaults]
registry_central_url="{url}"
[profiles.fixture]
display_name="Memory fixture"
repo_id="fixture"
provider="github"
repo="test/fixture"
local_path="{}"
artifact_root="{}"
default_target_branch="main"
"#,
            tmp.path().display(),
            tmp.path().display()
        ),
    )
    .unwrap();
    let role = role.to_owned();
    let request_role = role.clone();
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut request = Vec::new();
        let mut buf = [0; 4096];
        loop {
            let count = socket.read(&mut buf).unwrap();
            assert!(count > 0);
            request.extend_from_slice(&buf[..count]);
            if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                let header = String::from_utf8_lossy(&request[..end]);
                let length: usize = header
                    .lines()
                    .find_map(|line| {
                        line.to_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse().unwrap())
                    })
                    .unwrap();
                if request.len() >= end + 4 + length {
                    break;
                }
            }
        }
        let request = String::from_utf8(request).unwrap();
        assert!(request.starts_with(if request_role == "worker" {
            "POST /api/worker-memory/memories/list "
        } else {
            "POST /memories/list "
        }));
        assert!(request.contains(if request_role == "worker" {
            "Authorization: Bearer test-coordinator-key"
        } else {
            "Authorization: Bearer test-file-key"
        }));
        let body: serde_json::Value =
            serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["session_key"], "gah:manager:fixture");
        assert_eq!(body["limit"], 7);
        let body = r#"{"records":[],"total":0}"#;
        write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
    });
    let output = Command::new(env!("CARGO_BIN_EXE_gah"))
        .args([
            "memory",
            "list",
            "--profile",
            "fixture",
            "--limit",
            "7",
            "--config",
        ])
        .arg(config)
        .env("HOME", tmp.path())
        .env("GAH_CANONICAL_CONFIG", tmp.path().join("canonical.toml"))
        .env("GAH_NODE_ROLE", role)
        .env_remove("COORDINATOR_TOKEN")
        .env_remove("TDAI_GATEWAY_API_KEY")
        .env_remove("TDAI_GATEWAY_URL")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    server.join().unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()["total"],
        0
    );
}
