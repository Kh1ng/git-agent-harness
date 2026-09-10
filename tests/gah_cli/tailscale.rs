use super::*;

/// Issue #943: `gah tailscale-ip` prints the device's tailnet IPv4 so
/// operators never hardcode one into `registry_central_url` or pairing
/// commands. The authoritative source is `tailscale ip -4`; the fake bin
/// here stands in for it (both agree the machine is on the tailnet).
#[test]
fn tailscale_ip_prefers_the_authoritative_cli_output() {
    let tmp = test_tempdir();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    make_fake_bin_with_body(
        &bin_dir,
        "tailscale",
        "#!/bin/sh\nif [ \"$1\" = \"ip\" ] && [ \"$2\" = \"-4\" ]; then echo \"100.118.97.79\\n fd7a:115c:a1e0::1\"; exit 0; fi\nexit 1\n",
    );
    let config = tmp.path().join("config.toml");
    std::fs::write(&config, "[defaults]\n").unwrap();

    let output = bin()
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin_dir.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .args(["tailscale-ip", "--config", config.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "100.118.97.79"
    );

    let json = bin()
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin_dir.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .args([
            "tailscale-ip",
            "--config",
            config.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(json.status.success());
    let parsed: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(parsed["tailscale_ip"], "100.118.97.79");
}

/// A device with no tailscale CLI and no interface address inside the
/// tailnet range must fail with an actionable error, never print a LAN IP.
#[test]
fn tailscale_ip_fails_closed_off_tailnet() {
    let tmp = test_tempdir();
    // No `tailscale` binary on PATH; `ip`/`ifconfig` are real but their
    // addresses are outside 100.64.0.0/10, so the CIDR filter rejects them.
    // To keep the test hermetic even on a machine that IS on a tailnet,
    // shadow both interface tools with outputs that stay off-tailnet.
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    for tool in ["ip", "ifconfig"] {
        make_fake_bin_with_body(
            &bin_dir,
            tool,
            "#!/bin/sh\necho '9: eth0    inet 192.168.1.10/24 scope global eth0'\n",
        );
    }
    let config = tmp.path().join("config.toml");
    std::fs::write(&config, "[defaults]\n").unwrap();

    let output = bin()
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin_dir.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .args(["tailscale-ip", "--config", config.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no tailnet IPv4 address"), "got: {stderr}");
    assert!(
        !stderr.contains("192.168.1.10"),
        "must never surface a non-tailnet address"
    );
}
