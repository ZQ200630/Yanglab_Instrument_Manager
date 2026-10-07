#[path = "support/mdt_wire.rs"]
mod peer;
use peer::Wire;
use yang_drivers::mdt::parse_reply;
#[test]
fn echo_and_prompts_parse_all_reviewed_forms() {
    for raw in [
        b"answer\r*\r".as_slice(),
        b"answer\n*\n",
        b"answer\r\n*\r\n",
    ] {
        assert_eq!(
            parse_reply("id?", raw, Some(false)).unwrap().lines,
            ["answer"]
        );
    }
    let r = parse_reply("id?", b"id?\r\nMDT693B,1.23\r\n*\r\n", None).unwrap();
    assert!(r.echo_enabled);
    assert_eq!(r.lines, ["MDT693B,1.23"]);
    assert!(parse_reply("id?", b"answer\n*\n", Some(true)).is_err());
    assert!(parse_reply("id?", b"id?\nanswer\n*\n", Some(false)).is_err());
    assert!(parse_reply("id?", b"!\n", None).is_err());
    assert!(parse_reply("id?", b"answer\n*\nextra", None).is_err());
    assert_eq!(
        parse_reply("friendly?", b"\r\n*\r\n", Some(false))
            .unwrap()
            .lines,
        [""]
    );
}
#[test]
fn response_is_bounded_to_4096() {
    let mut raw = vec![b'a'; 4092];
    raw.extend(b"\n*\n");
    assert!(parse_reply("id?", &raw, Some(false)).is_ok());
    raw.insert(0, b'a');
    raw.insert(0, b'a');
    assert!(parse_reply("id?", &raw, Some(false)).is_err());
}
#[test]
fn native_parser_preserves_echo_and_deferred_prompt_lf() {
    for echo in [false, true] {
        for ending in ["\r", "\n", "\r\n"] {
            let w = Wire::new();
            {
                let mut d = w.data.lock().unwrap();
                d.echo = echo;
                d.ending = ending.into();
            }
            let mut m = w.driver();
            m.connect().unwrap();
            if ending == "\r" {
                w.data.lock().unwrap().reply.push_back(b'\n');
            }
            assert_eq!(m.get_serial_number().unwrap(), "2110148249-10");
            m.close().unwrap();
        }
    }
}
#[test]
fn stale_bytes_reject_before_next_query_is_written() {
    let w = Wire::new();
    let mut m = w.driver();
    m.connect().unwrap();
    w.data.lock().unwrap().reply.extend(b"stale");
    let n = w.commands().len();
    assert!(m.get_serial_number().is_err());
    assert_eq!(w.commands().len(), n);
    m.close().unwrap();
}

#[test]
fn deferred_lf_counts_toward_previous_reply_bound() {
    let w = Wire::new();
    let mut reply = vec![b'a'; 4093];
    reply.extend(b"\r*\r");
    {
        let mut d = w.data.lock().unwrap();
        d.ending = "\r".into();
        d.overrides.insert("friendly?".into(), reply);
        d.prefix_lf_on = Some("echo?".into());
    }
    let mut m = w.driver();
    assert!(m.connect().is_err());
    assert!(!m.has_resource_responsibility());
}
