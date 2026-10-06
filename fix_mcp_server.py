import re

with open("tests/mcp_server.rs", "r") as f:
    content = f.read()

target = """    assert_eq!(tool("gah_info")["title"], "GAH server info");
    assert_eq!(
        tool("gah_info")["description"],
        "Identify the connected GAH control-plane node and API version."
    );
    assert_eq!(
        tool("gah_route_approval_revoke")["title"],
        "GAH paid-route approval revoke"
    );"""

replacement = """    let expected_metadata = [
        ("gah_info", "GAH server info", "Identify the connected GAH control-plane node and API version."),
        ("gah_cli_router", "GAH CLI router status", "Read-only snapshot of the CLI Proxy API connection status, settings, quotas, and allowed models."),
        ("gah_status", "GAH status", "Full status snapshot for a profile: merge requests, blockers, availability, ledger summary."),
        ("gah_quota", "GAH quota snapshot", "Usage/quota snapshot for a profile over a time window."),
        ("gah_usage_rollup", "GAH usage rollup", "Actual manager-chat usage by day, backend, and model; use days=30 for a monthly view."),
        ("gah_doctor", "GAH doctor", "Run readiness checks for a profile (auth, config, backend availability)."),
        ("gah_report", "GAH report", "Aggregate usage/cost/success-rate report, optionally grouped by backend or model."),
        ("gah_profiles", "List GAH profiles", "List all configured GAH profiles."),
        ("gah_work_history", "Work item ledger history", "Full chronological ledger history (all attempts) for one work item."),
        ("gah_sync", "GAH sync", "Classified open (and recently resolved) merge requests/pull requests for a profile."),
        ("gah_ledger_summary", "GAH ledger summary", "Aggregate ledger counts (success/fail, by mode/backend/model, token usage) over a window."),
        ("gah_ledger_clear_attempts", "Clear ledger attempts", "Append a tombstone ledger entry so a stuck work_id becomes dispatchable again."),
        ("gah_availability", "GAH availability", "Durable backend/model availability state, global (not per-profile)."),
        ("gah_availability_clear", "Clear availability override", "Override a stale unavailable record once the backend is confirmed healthy again."),
        ("gah_hold", "List review holds", "Work IDs currently under an out-of-band manager review hold for a profile."),
        ("gah_hold_set", "Set a review hold", "Mark a work_id as under active out-of-band manager review; gah's auto-merge loop will skip it."),
        ("gah_hold_clear", "Clear a review hold", "Release a previously set review hold on a work_id."),
        ("gah_events", "GAH events", "Recent controller and dispatch events for a profile."),
        ("gah_controller_activity", "GAH controller activity", "Summarized agent/controller activity for a profile."),
        ("gah_loop_status", "GAH loop status", "Report whether the autonomous GAH loop is running for a profile."),
        ("gah_dispatch", "Dispatch a GAH job", "Submit a dispatch as a fleet session and wait for its terminal push event by default. Set waitForCompletion=false to return immediately with the running session."),
        ("gah_route_approvals", "GAH paid-route approvals", "List pending and active paid-route approval requests for a profile (state, exact scope, consumption)."),
        ("gah_route_approval_grant", "GAH paid-route approval grant", "Grant one exact paid backend/model route for one work item. The scope must match the pending request exactly — it cannot be broadened here."),
        ("gah_route_approval_revoke", "GAH paid-route approval revoke", "Revoke a previously granted paid-route approval for one exact scope."),
    ];
    for (name, title, desc) in expected_metadata {
        assert_eq!(tool(name)["title"], title, "{name}");
        assert_eq!(tool(name)["description"], desc, "{name}");
    }"""

new_content = content.replace(target, replacement)
if new_content == content:
    print("Could not find target block to replace!")
else:
    with open("tests/mcp_server.rs", "w") as f:
        f.write(new_content)
    print("Replaced successfully!")
