/// Refuse to adopt a process whose own startup identity disagrees with the
/// requested mode or the disconnected, versioned protocol contract.
#[cfg(test)]
pub(crate) fn validate_startup_handshake(
    requested_mode: &str,
    actual_mode: Option<&str>,
    protocol_version: Option<u64>,
    connected: Option<bool>,
) -> Result<(), String> {
    validate_versioned_handshake(2, requested_mode, actual_mode, protocol_version, connected)
}
#[cfg(test)]
pub(crate) fn validate_versioned_handshake(
    requested_version: u64,
    requested_mode: &str,
    actual_mode: Option<&str>,
    protocol_version: Option<u64>,
    connected: Option<bool>,
) -> Result<(), String> {
    if requested_mode != "real" || actual_mode != Some("real") {
        return Err("Python worker mode did not match the requested mode".to_string());
    }
    if ![2, 3].contains(&requested_version) || protocol_version != Some(requested_version) {
        return Err(format!(
            "Python worker protocol identity did not match version {requested_version}"
        ));
    }
    if connected != Some(false) {
        return Err("Python worker did not confirm a disconnected startup".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn handshake_requires_requested_version() {
        assert!(
            validate_versioned_handshake(3, "real", Some("real"), Some(2), Some(false)).is_err()
        );
        assert!(
            validate_versioned_handshake(3, "real", Some("real"), Some(3), Some(false)).is_ok()
        );
        assert!(
            validate_versioned_handshake(2, "real", Some("real"), Some(3), Some(false)).is_err()
        );
    }

    #[test]
    fn startup_rejects_obsolete_worker_selection() {
        assert!(
            validate_startup_handshake("simulate", Some("real"), Some(2), Some(false)).is_err()
        );
    }

    #[test]
    fn startup_rejects_missing_or_wrong_protocol_identity() {
        assert!(validate_startup_handshake("real", Some("real"), None, Some(false)).is_err());
        assert!(validate_startup_handshake("real", Some("real"), Some(1), Some(false)).is_err());
    }

    #[test]
    fn startup_rejects_a_worker_that_claims_an_existing_connection() {
        assert!(validate_startup_handshake("real", Some("real"), Some(2), Some(true)).is_err());
        assert!(validate_startup_handshake("real", Some("real"), Some(2), None).is_err());
    }

    #[test]
    fn startup_accepts_a_matching_disconnected_worker() {
        assert!(
            validate_startup_handshake("simulate", Some("simulate"), Some(2), Some(false)).is_err()
        );
        assert!(validate_startup_handshake("real", Some("real"), Some(2), Some(false)).is_ok());
    }
}
