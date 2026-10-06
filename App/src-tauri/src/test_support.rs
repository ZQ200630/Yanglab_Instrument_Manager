//! Test process configuration only; never compiled into an instrument backend.
use std::ffi::OsStr;
use std::path::PathBuf;

fn resolve_visa_python(value: Option<&OsStr>) -> Result<PathBuf, String> {
    let help = "Set YANG_LAB_TEST_PYTHON to the absolute python.exe in the existing Anaconda VISA environment";
    let path = value.map(PathBuf::from).ok_or(help)?;
    if !path.is_absolute()
        || !path.file_name().is_some_and(|name| name.eq_ignore_ascii_case("python.exe"))
        || !path.parent().and_then(|p| p.file_name()).is_some_and(|name| name.eq_ignore_ascii_case("VISA"))
    {
        return Err(help.into());
    }
    Ok(path)
}

pub(crate) fn visa_python() -> PathBuf {
    let value = std::env::var_os("YANG_LAB_TEST_PYTHON");
    let path = resolve_visa_python(value.as_deref()).expect("Invalid native test configuration");
    assert!(path.is_file(), "VISA test executable does not exist: {}", path.display());
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_an_explicit_absolute_visa_environment_with_spaces() {
        let path = r"C:\Lab Tools\Anaconda\envs\VISA\python.exe";
        assert_eq!(resolve_visa_python(Some(OsStr::new(path))).unwrap(), PathBuf::from(path));
    }

    #[test]
    fn missing_relative_and_non_visa_interpreters_fail_with_configuration_help() {
        for value in [None, Some("envs/VISA/python.exe"), Some(r"C:\Anaconda\python.exe")] {
            let error = resolve_visa_python(value.map(OsStr::new)).unwrap_err();
            assert!(error.contains("YANG_LAB_TEST_PYTHON"), "{error}");
        }
    }
}
