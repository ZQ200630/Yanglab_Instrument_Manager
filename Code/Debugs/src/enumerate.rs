use serde_json::{json, Value};
pub fn inventory() -> Result<Value, String> {
    use yang_drivers::transport::{serial_discovery::enumerate_serial, ResourceBook, VisaManager};
    let serial = enumerate_serial().map_err(|e| e.to_string());
    let visa = match VisaManager::load_system(ResourceBook::default()) {
        Ok(manager) => {
            let result = manager.enumerate().map_err(|e| e.to_string());
            let close = manager.release().map_err(|e| e.error.to_string());
            match (result, close) {
                (Ok(v), Ok(_)) => Ok(v),
                (Err(e), _) | (_, Err(e)) => Err(e),
            }
        }
        Err(e) => Err(e.to_string()),
    };
    Ok(
        json!({"serial":serial.as_ref().ok(),"serial_error":serial.as_ref().err(),"visa":visa.as_ref().ok(),"visa_error":visa.as_ref().err(),"instrument_sessions_opened":false}),
    )
}
