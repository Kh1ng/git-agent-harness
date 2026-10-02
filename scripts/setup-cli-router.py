#!/usr/bin/env python3
"""Install a private CLIProxyAPI sidecar and bind it to GAH's OpenCode runner."""
import argparse
from datetime import datetime, timedelta, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import secrets
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import urllib.request
import urllib.parse


VERSION = "8.0.10"


def import_agy_account(directory, home, label):
    """Verify AGY's refresh credential with the proxy client before copying it."""
    target = directory / "router-auth" / f"antigravity-{label}.json"
    if target.exists():
        print(f"Kept existing router account {label}.")
        return
    token = json.loads((home / ".gemini/antigravity-cli/antigravity-oauth-token").read_text())["token"]
    # Use the public client distributed by the exact upstream release, without
    # copying its OAuth constants into this repository.
    source = "https://raw.githubusercontent.com/router-for-me/CLIProxyAPI/6fecc6e5567912661654a4eaf9b8f5436facd1c2/internal/auth/antigravity/constants.go"
    with urllib.request.urlopen(source, timeout=30) as response:
        metadata = response.read(64 * 1024).decode()
    client = {}
    for field in ["ClientID", "ClientSecret"]:
        match = re.search(r'\b' + field + r'\s*=\s*"([^"\r\n]{1,512})"', metadata)
        if not match:
            raise ValueError("Cannot read the pinned CLIProxyAPI OAuth client. No credential was copied.")
        client[field] = match.group(1)
    form = urllib.parse.urlencode({
        "client_id": client["ClientID"], "client_secret": client["ClientSecret"],
        "grant_type": "refresh_token", "refresh_token": token["refresh_token"],
    }).encode()
    with urllib.request.urlopen(urllib.request.Request("https://oauth2.googleapis.com/token", data=form), timeout=30) as response:
        refreshed = json.load(response)
    headers = {"Authorization": "Bearer " + refreshed["access_token"], "Content-Type": "application/json", "User-Agent": "antigravity/cli/1.0.13 (aidev_client; os_type=linux; arch=amd64)"}
    request = urllib.request.Request("https://www.googleapis.com/oauth2/v2/userinfo?alt=json", headers=headers)
    with urllib.request.urlopen(request, timeout=30) as response:
        email = json.load(response)["email"]
    request = urllib.request.Request("https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist", headers=headers, data=b'{"metadata":{"ideType":"ANTIGRAVITY"}}')
    with urllib.request.urlopen(request, timeout=30) as response:
        project = json.load(response).get("cloudaicompanionProject")
    if isinstance(project, dict):
        project = project.get("id")
    if not isinstance(project, str) or not project:
        raise ValueError("AGY account has no active project. Use CLIProxyAPI OAuth login; no credential was copied.")
    now = datetime.now(timezone.utc)
    credential = {
        "type": "antigravity", "label": label, "email": email, "project_id": project,
        "access_token": refreshed["access_token"], "refresh_token": refreshed.get("refresh_token", token["refresh_token"]),
        "expires_in": refreshed["expires_in"], "timestamp": int(now.timestamp() * 1000),
        "expired": (now + timedelta(seconds=refreshed["expires_in"])).isoformat(),
    }
    atomic_write(target, json.dumps(credential, indent=2) + "\n")
    print(f"Imported router account {label}; original AGY login unchanged.")


def atomic_write(path, text, mode=0o600):
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    fd, temporary = tempfile.mkstemp(dir=path.parent)
    try:
        os.fchmod(fd, mode)
        with os.fdopen(fd, "w") as stream:
            stream.write(text)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def download_binary(directory, version):
    system = {"Darwin": "darwin", "Linux": "linux"}.get(platform.system())
    arch = {"arm64": "aarch64", "aarch64": "aarch64", "x86_64": "amd64"}.get(platform.machine())
    if not system or not arch:
        raise ValueError("Use this installer on macOS or Linux, including WSL.")
    asset = f"CLIProxyAPI_{version}_{system}_{arch}.tar.gz"
    base = f"https://github.com/router-for-me/CLIProxyAPI/releases/download/v{version}"
    with tempfile.TemporaryDirectory() as temporary:
        archive = Path(temporary) / asset
        with urllib.request.urlopen(f"{base}/{asset}", timeout=30) as response, archive.open("wb") as target:
            shutil.copyfileobj(response, target)
        with urllib.request.urlopen(f"{base}/checksums.txt", timeout=30) as response:
            checksums = response.read().decode().splitlines()
        expected = next((line.split()[0] for line in checksums if line.split() and line.split()[-1].lstrip("*") == asset), None)
        if not expected:
            raise ValueError("CLIProxyAPI release checksum entry is missing.")
        if hashlib.sha256(archive.read_bytes()).hexdigest() != expected:
            raise ValueError("CLIProxyAPI release checksum mismatch.")
        with tarfile.open(archive) as bundle:
            member = bundle.getmember("cli-proxy-api")
            if not member.isfile():
                raise ValueError("Release executable is not a regular file.")
            binary = bundle.extractfile(member).read()
        directory.mkdir(parents=True, exist_ok=True, mode=0o700)
        target = directory / "cli-proxy-api"
        fd, temporary = tempfile.mkstemp(dir=directory)
        try:
            os.fchmod(fd, 0o700)
            with os.fdopen(fd, "wb") as stream:
                stream.write(binary)
                stream.flush()
                os.fsync(stream.fileno())
            os.replace(temporary, target)
        finally:
            if os.path.exists(temporary):
                os.unlink(temporary)
    return target


def configure(directory, port):
    """Reinstall without rotating keys or replacing the operator's router policy."""
    settings_path = directory / "cli-router.json"
    if settings_path.exists():
        settings = json.loads(settings_path.read_text())
        if not isinstance(settings, dict) or not all(isinstance(settings.get(k), str) and settings[k] for k in ["url", "apiKey", "managementKey"]):
            raise ValueError("Existing CLI router settings are invalid; restore them before reinstalling.")
        if settings["url"] != f"http://127.0.0.1:{port}":
            raise ValueError("Existing connection uses another URL; choose its port or a separate --directory.")
    else:
        settings = {"url": f"http://127.0.0.1:{port}", "apiKey": secrets.token_urlsafe(32), "managementKey": secrets.token_urlsafe(32)}
        atomic_write(settings_path, json.dumps(settings, indent=2) + "\n")
    settings_path.chmod(0o600)
    auth = directory / "router-auth"
    auth.mkdir(parents=True, exist_ok=True, mode=0o700)
    auth.chmod(0o700)
    config = directory / "router.yaml"
    if not config.exists():
        # JSON strings are valid YAML scalars, including paths with spaces.
        scalar = json.dumps
        atomic_write(config, f'''config-version: 8
server:
  host: "127.0.0.1"
  port: {port}
management:
  allow-remote: false
  secret-key: {scalar(settings["managementKey"])}
  disable-auto-update-panel: true
access:
  api-keys:
    - {scalar(settings["apiKey"])}
oauth:
  auth-dir: {scalar(str(auth))}
routing:
  strategy: "round-robin"
  session-affinity: true
  session-affinity-ttl: "1h"
  session-affinity-subagents: true
  retry:
    request-retry: 2
    max-retry-interval: 5
observability:
  logs:
    debug: false
    logging-to-file: false
    request-log: false
''')
    config.chmod(0o600)
    return settings


def configure_opencode(directory, settings, executable, models):
    """Keep provider credentials out of argv and isolate the pooled runner's state."""
    config_path = directory / "router-opencode.json"
    config = {
        "$schema": "https://opencode.ai/config.json",
        "share": "disabled",
        "enabled_providers": ["gah-router"],
        "provider": {"gah-router": {
            "npm": "@ai-sdk/openai-compatible", "name": "CLI subscription router",
            "options": {"baseURL": settings["url"] + "/v1"},
            "models": {model: {"name": model} for model in models},
        }},
        "agent": {
            "gah-implementer": {"description": "GAH implementation runner", "mode": "primary"},
            "gah-reviewer": {"description": "GAH review runner", "mode": "primary", "permission": {"edit": "deny"}},
        },
    }
    atomic_write(config_path, json.dumps(config, indent=2) + "\n")
    wrapper = directory / "gah-cli-router-opencode"
    (directory / "router-runner-home").mkdir(exist_ok=True, mode=0o700)
    # Read the current connection and inventory at launch so dashboard changes
    # apply to both CLI jobs and ACP sessions without stale credentials.
    atomic_write(wrapper, f'''#!{sys.executable}
import json, os, sys, urllib.request
from pathlib import Path
try:
    class NoRedirect(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, *args, **kwargs):
            return None
    settings = json.loads(Path({str(directory / "cli-router.json")!r}).read_text())
    config = json.loads(Path({str(config_path)!r}).read_text())
    request = urllib.request.Request(settings["url"] + "/v1/models", headers={{"Authorization": "Bearer " + settings["apiKey"]}})
    with urllib.request.build_opener(NoRedirect).open(request, timeout=12) as response:
        models = json.loads(response.read(2_000_001))["data"]
    config["provider"]["gah-router"]["options"] = {{"baseURL": settings["url"] + "/v1", "apiKey": settings["apiKey"]}}
    config["provider"]["gah-router"]["models"] = {{m["id"]: {{"name": m["id"]}} for m in models}}
    inherited = json.loads(os.environ.get("OPENCODE_CONFIG_CONTENT", "{{}}"))
    config.update({{k: v for k, v in inherited.items() if k not in ["provider", "enabled_providers", "share"]}})
    os.environ["OPENCODE_CONFIG_CONTENT"] = json.dumps(config)
    os.environ["OPENCODE_CONFIG"] = {str(config_path)!r}
    os.environ["OPENCODE_DISABLE_SHARE"] = "true"
except Exception:
    sys.exit("CLI router is unavailable. Check its connection and subscription accounts in GAH Quota.")
os.execv({executable!r}, [{executable!r}, *sys.argv[1:]])
''', 0o700)
    return wrapper


def register_instance(path, profile, directory, wrapper):
    """Append one explicit instance, preserving every existing candidate and default."""
    original = path.read_text()
    config = tomllib.loads(original)
    if profile not in config.get("profiles", {}):
        raise ValueError("Choose an existing GAH profile.")
    instance = config["profiles"][profile].get("routing", {}).get("backend_instances", {}).get("cli-router")
    if instance is not None:
        if instance.get("executable") != str(wrapper) or instance.get("runner_kind") != "opencode":
            raise ValueError("A different cli-router instance already exists; no configuration was changed.")
        return
    scalar = json.dumps
    section = f'''\n[profiles.{scalar(profile)}.routing.backend_instances.cli-router]
runner_kind = "opencode"
logical_backend = "opencode"
executable = {scalar(str(wrapper))}
state_root = {scalar(str(directory / "router-runner-home"))}
account_label = "cli-router"
auth_source_label = "subscription-router"
quota_pool = "cli-router"
enabled = true
'''
    tomllib.loads(original + section)
    atomic_write(path, original + section)


def install_service(directory, binary):
    if platform.system() == "Linux":
        unit = Path.home() / ".config/systemd/user/gah-cli-router.service"
        # systemd's quoted words do not use shell quoting. Percent is a unit specifier.
        quote = lambda p: json.dumps(str(p).replace("%", "%%"))
        atomic_write(unit, f'''[Unit]
Description=GAH CLI subscription router
After=network-online.target
[Service]
ExecStart={quote(binary)} -config {quote(directory / "router.yaml")}
Restart=on-failure
RestartSec=5
UMask=0077
[Install]
WantedBy=default.target
''')
        subprocess.run(["systemctl", "--user", "daemon-reload"], check=True)
        subprocess.run(["systemctl", "--user", "enable", "gah-cli-router"], check=True)
        subprocess.run(["systemctl", "--user", "restart", "gah-cli-router"], check=True)
    elif platform.system() == "Darwin":
        import plistlib
        path = Path.home() / "Library/LaunchAgents/tech.coltonspurgin.gah-cli-router.plist"
        contents = plistlib.dumps({"Label": "tech.coltonspurgin.gah-cli-router", "ProgramArguments": [str(binary), "-config", str(directory / "router.yaml")], "RunAtLoad": True, "KeepAlive": True, "Umask": 0o077})
        atomic_write(path, contents.decode())
        subprocess.run(["launchctl", "bootout", f"gui/{os.getuid()}", str(path)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        subprocess.run(["launchctl", "bootstrap", f"gui/{os.getuid()}", str(path)], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path, default=Path.home() / ".config/gah")
    parser.add_argument("--version", default=VERSION)
    parser.add_argument("--port", type=int, default=8317)
    parser.add_argument("--opencode", default=shutil.which("opencode"))
    parser.add_argument("--register-profile")
    parser.add_argument("--gah-config", type=Path, default=Path.home() / ".config/gah/config.toml")
    parser.add_argument("--service", action="store_true")
    parser.add_argument("--import-agy-home", type=Path, action="append", default=[], help="Reuse a verified AGY login from this HOME; repeat for isolated accounts.")
    parser.add_argument("--refresh-models", action="store_true", help="Refresh OpenCode models from an already-running proxy; do not download or restart it.")
    args = parser.parse_args()
    if not 1 <= args.port <= 65535 or not args.version.replace(".", "").isdigit():
        parser.error("Choose a valid port and numeric release version.")
    if not args.opencode or not os.access(args.opencode, os.X_OK):
        parser.error("Install OpenCode or pass --opencode /absolute/path/to/opencode.")
    directory = args.directory.expanduser().resolve()
    settings = configure(directory, args.port)
    for index, home in enumerate(args.import_agy_home, 1):
        import_agy_account(directory, home.expanduser().resolve(), f"agy-{index}")
    binary = directory / "bin/cli-proxy-api"
    if not args.refresh_models:
        binary = download_binary(directory / "bin", args.version)
        if args.service:
            install_service(directory, binary)
    models = []
    try:
        request = urllib.request.Request(settings["url"] + "/v1/models", headers={"Authorization": "Bearer " + settings["apiKey"]})
        with urllib.request.urlopen(request, timeout=10) as response:
            models = [item["id"] for item in json.load(response)["data"] if isinstance(item.get("id"), str)]
    except (OSError, ValueError, KeyError):
        if args.refresh_models:
            raise ValueError("Cannot fetch router models; existing OpenCode configuration was preserved.") from None
    wrapper = configure_opencode(directory, settings, str(Path(args.opencode).resolve()), models)
    if args.register_profile:
        register_instance(args.gah_config.expanduser(), args.register_profile, directory, wrapper)
    print(f"CLI router configured at {settings['url']}; {len(models)} models registered.")
    print(f"Runner: {wrapper}")
    print("Add subscription accounts with the proxy's OAuth login, then rerun with --refresh-models.")


if __name__ == "__main__":
    main()
