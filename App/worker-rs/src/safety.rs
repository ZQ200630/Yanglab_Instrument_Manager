use yang_protocol::RequestV3;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SafetyIntent {
    Zero,
    CurrentOff,
    TecOff,
    Disconnect,
    ScopeStop,
    GeneratorOff(u8),
}
pub fn intent(driver: &str, request: &RequestV3) -> Option<SafetyIntent> {
    if request.method == "disconnect" {
        return Some(SafetyIntent::Disconnect);
    }
    if request.method != "action" {
        return None;
    }
    match (driver, request.params["name"].as_str()) {
        ("voltage", Some("zero")) => Some(SafetyIntent::Zero),
        ("gain", Some("disable_current")) => Some(SafetyIntent::CurrentOff),
        ("gain", Some("disable_tec")) => Some(SafetyIntent::TecOff),
        ("scope", Some("stop")) => Some(SafetyIntent::ScopeStop),
        ("generator", Some("set_output")) if request.params["args"]["enabled"]==false =>
            request.params["args"]["channel"].as_u64().filter(|n|(1..=2).contains(n)).map(|n|SafetyIntent::GeneratorOff(n as u8)),
        _ => None,
    }
}
