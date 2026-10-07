use yang_protocol::RequestV3;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SafetyIntent {
    Zero,
    CurrentOff,
    TecOff,
    Disconnect,
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
        _ => None,
    }
}
