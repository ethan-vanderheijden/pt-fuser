use std::process::Command;

const BINARIES: [(&str, &str); 3] = [
    ("merge", env!("CARGO_BIN_EXE_merge")),
    ("histogram", env!("CARGO_BIN_EXE_histogram")),
    ("convert_perfetto", env!("CARGO_BIN_EXE_convert_perfetto")),
];

#[test]
fn version_flags_work_without_positional_arguments() {
    // The Git commit is optional, so the version is either bare or suffixed.
    let version = pt_fuser::VERSION;
    assert!(
        version == env!("CARGO_PKG_VERSION")
            || version.starts_with(concat!(env!("CARGO_PKG_VERSION"), " (")),
        "unexpected version string: {version}"
    );
    for (name, binary) in BINARIES {
        let expected = format!("pt-fuser {name} {version}\n");
        for flag in ["-v", "--version"] {
            let output = Command::new(binary).arg(flag).output().unwrap();
            assert!(output.status.success(), "{binary} {flag}: {output:?}");
            assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
            assert!(output.stderr.is_empty());
        }
    }
}

#[test]
fn ordinary_invocations_do_not_require_the_version_flag() {
    // A readable non-trace file lets us check that parsing reaches the existing
    // trace validation instead of failing on an absent version flag.
    let input = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
    let arguments = [
        vec!["", input, input],
        vec!["errors", input],
        vec![input, ""],
    ];
    for ((_, binary), args) in BINARIES.into_iter().zip(arguments) {
        let output = Command::new(binary).args(args).output().unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(!output.status.success());
        assert!(
            stderr.contains("version delimiter is incorrect"),
            "{binary} did not reach trace validation: {stderr}"
        );
    }
}
