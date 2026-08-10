use std::process::Command;

#[test]
fn cli_exposes_operator_inspection_and_control_commands() {
    let output = Command::new(env!("CARGO_BIN_EXE_pitools"))
        .arg("--help")
        .output()
        .expect("run pitools help");
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    for command in ["watchlist", "events", "audit", "inspect", "cancel"] {
        assert!(help.contains(command), "missing CLI command: {command}");
    }
}
