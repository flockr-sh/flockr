use super::harness::*;

fn wait_for_blocked_or_done(
    socket_path: &Path,
    pane_id: &str,
    timeout: &str,
) -> std::process::Output {
    run_cli(
        socket_path,
        &[
            "agent",
            "wait",
            pane_id,
            "--until",
            "blocked",
            "--until",
            "done",
            "--timeout",
            timeout,
        ],
    )
}

#[test]
fn agent_wait_accepts_repeated_until_and_exits_when_one_status_matches() {
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let socket_path = runtime_dir.join("flockr.sock");

    let flockr = spawn_flockr(&config_home, &runtime_dir, &socket_path);
    wait_for_socket(&socket_path, Duration::from_secs(5));

    let created = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"req_cli_2","method":"workspace.create","params":{{"cwd":"{}","focus":true}}}}"#,
            base.display()
        ),
    );
    let pane_id = created["result"]["root_pane"]["pane_id"]
        .as_str()
        .unwrap()
        .to_string();

    let report_agent = |state| {
        run_cli(
            &socket_path,
            &[
                "pane",
                "report-agent",
                &pane_id,
                "--source",
                "custom:test",
                "--agent",
                "pi",
                "--state",
                state,
            ],
        )
        .status
        .success()
    };
    // Neither listed status matches while the agent is working.
    assert!(report_agent("working"));
    let still_working = wait_for_blocked_or_done(&socket_path, &pane_id, "100");
    assert!(!still_working.status.success());
    assert!(
        String::from_utf8_lossy(&still_working.stderr)
            .contains("timed out waiting for agent status"),
        "stderr: {}",
        String::from_utf8_lossy(&still_working.stderr)
    );

    let wait_socket = socket_path.clone();
    let wait_pane = pane_id.clone();
    let waiter = thread::spawn(move || wait_for_blocked_or_done(&wait_socket, &wait_pane, "10000"));
    assert!(report_agent("idle"));

    let waited = waiter.join().unwrap();
    assert!(
        waited.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&waited.stderr)
    );
    let waited_json: serde_json::Value = serde_json::from_slice(&waited.stdout).unwrap();
    // Nobody is looking at the pane, so the finished agent stays done.
    assert_eq!(waited_json["result"]["agent"]["agent_status"], "done");
    assert_eq!(waited_json["result"]["agent"]["agent"], "pi");

    cleanup_spawned_flockr(flockr, base);
}
