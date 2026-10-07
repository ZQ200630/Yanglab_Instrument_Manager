#[test]
fn remote_stage_requires_saved_trust_and_cannot_request_instrument_actions() {
    let out = std::env::temp_dir()
        .join("Result/remote-check")
        .to_str()
        .unwrap()
        .to_owned();
    let args = vec![
        "remote".into(),
        "--stage".into(),
        "readonly".into(),
        "--out".into(),
        out,
    ];
    assert!(yang_debug::parse_args(&args).is_err());
    let mut saved = args;
    saved.extend([
        "--peers".into(),
        "C:/private/peers.dpapi".into(),
        "--host".into(),
        "a".repeat(32),
    ]);
    assert!(!yang_debug::parse_args(&saved).unwrap().execute_requested());
    saved.extend([
        "--actions".into(),
        "[{\"name\":\"connect\",\"args\":{}}]".into(),
    ]);
    assert!(yang_debug::parse_args(&saved).is_err());
}
