use serde_json::{json, Value};
use std::io::Cursor;
use yang_protocol::*;
const SESSION: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
fn ping(params: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({"v":3,"id":"test","method":"ping","params":params,"context":null}))
        .unwrap()
}
fn startup() -> NativeIdentity {
    NativeIdentity {
        startup_revision: 1,
        worker_kind: "rust".into(),
        executable: "C:/Yang Lab/yang-worker.exe".into(),
        package_revision: "test-1".into(),
        protocol_version: 3,
        mode: "real".into(),
        session_id: SESSION.into(),
        activated: false,
        connected: false,
        domains: vec![],
    }
}
#[test]
fn vectors_match_current_v3_contract() {
    let vectors: Vec<Value> =
        serde_json::from_str(include_str!("../../tests/fixtures/v3-contracts.json")).unwrap();
    for vector in vectors {
        let result = parse_request(vector["line"].as_str().unwrap().as_bytes());
        assert_eq!(
            result.is_ok(),
            vector["valid"].as_bool().unwrap(),
            "{}: {result:?}",
            vector["name"]
        );
        if let Ok(request) = result {
            assert_eq!(request.method, vector["method"].as_str().unwrap());
        }
    }
}
#[test]
fn strict_json_bounds() {
    assert!(parse_request(&ping(json!({}))).is_ok());
    for input in [
        br#"{"v":3,"v":3}"#.as_slice(),
        b"[]",
        b"\xff",
        b"{\"v\":3}\n{}",
    ] {
        assert!(parse_request(input).is_err());
    }
    let valid = String::from_utf8(ping(json!({}))).unwrap();
    let padded = format!("{valid}{}", " ".repeat(65536 - valid.len()));
    assert!(parse_request(padded.as_bytes()).is_ok());
    assert!(parse_request(format!("{padded} ").as_bytes()).is_err());
    for bad in [
        json!({"unexpected":null}),
        json!({"n":9007199254740992u64}),
        json!({"n":-9007199254740992i64}),
    ] {
        assert!(parse_request(&ping(bad)).is_err());
    }
    let nested = format!(
        r#"{{"v":3,"id":"x","method":"action","params":{{"name":"read","args":{{"x":{}0{}}}}},"context":{{"session_id":"{SESSION}","domain":{{"kind":"device","id":"11111111111111111111111111111111"}},"connection_id":null,"epoch":0}}}}"#,
        "[".repeat(33),
        "]".repeat(33)
    );
    assert!(parse_request(nested.as_bytes()).is_err());
}
#[test]
fn terminal_error_consistency() {
    let completed = OutcomeV3 {
        phase: Phase::Completed,
        context: None,
        result: Some(json!({"released":false})),
        error: None,
    };
    let encoded = encode_outcome("x", &completed).unwrap();
    assert_eq!(encoded.last(), Some(&b'\n'));
    let wire: Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(
        wire,
        json!({"v":3,"id":"x","ok":true,"phase":"completed","context":null,"result":{"released":false}})
    );
    let error = WireError {
        kind: "Timeout".into(),
        message: "retained".into(),
        attempt_id: None,
    };
    for phase in [
        Phase::RejectedBeforeCall,
        Phase::SupersededBeforeCall,
        Phase::CompletedReadbackFailed,
        Phase::FailedAfterCallStarted,
    ] {
        let outcome = OutcomeV3 {
            phase,
            context: None,
            result: None,
            error: Some(error.clone()),
        };
        let wire: Value = serde_json::from_slice(&encode_outcome("x", &outcome).unwrap()).unwrap();
        assert_eq!(wire["ok"], false);
        assert!(wire.get("result").is_none());
        assert!(encode_outcome(
            "x",
            &OutcomeV3 {
                result: Some(json!({})),
                ..outcome
            }
        )
        .is_err());
    }
    assert!(encode_outcome(
        "x",
        &OutcomeV3 {
            error: Some(error),
            ..completed
        }
    )
    .is_err());
}
#[test]
fn native_identity_is_disarmed() {
    assert!(startup().validate_startup().is_ok());
    assert!(NativeIdentity {
        worker_kind: "python".into(),
        ..startup()
    }
    .validate_startup()
    .is_err());
    assert!(NativeIdentity {
        activated: true,
        ..startup()
    }
    .validate_startup()
    .is_err());
    assert!(NativeIdentity {
        connected: true,
        ..startup()
    }
    .validate_startup()
    .is_err());
    assert!(NativeIdentity {
        domains: vec![json!({})],
        ..startup()
    }
    .validate_startup()
    .is_err());
    assert_eq!(Limits::default().max_domains, 64);
    assert_eq!(Limits::default().max_pending_normal, 31);
    assert_eq!(Limits::default().max_reply_slots, 226);
    assert_eq!(Limits::default().max_responsibility_slots, 225);
    assert_eq!(Limits::default().ordinary_slots, 4);
    assert_eq!(Limits::default().observation_slots, 4);
}
#[test]
fn bounded_line_reader_never_accepts_partial_or_extra_frame() {
    let mut reader = Cursor::new(b"{}\n{\"next\":1}\n".as_slice());
    assert_eq!(read_frame(&mut reader, 16).unwrap(), Some(b"{}\n".to_vec()));
    assert_eq!(
        read_frame(&mut reader, 16).unwrap(),
        Some(b"{\"next\":1}\n".to_vec())
    );
    assert_eq!(read_frame(&mut reader, 16).unwrap(), None);
    assert!(read_frame(&mut Cursor::new(b"{}"), 16).is_err());
    assert!(read_frame(&mut Cursor::new(b"12345678\n"), 8).is_err());
}
