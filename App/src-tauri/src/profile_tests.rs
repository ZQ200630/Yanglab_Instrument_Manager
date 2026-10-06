use super::*;
#[test]
fn default_profile_owns_local_host_but_observer_cannot_start_physical_worker() {
    let local=Profile::from_args(Vec::<String>::new()).unwrap();
    assert!(!local.network_only);
    let viewer=Profile::from_args(vec!["--network-only".into(),"--profile".into(),"observer".into()]).unwrap();
    assert!(viewer.require_hardware_host().is_err());
    let base=std::path::PathBuf::from("C:/AppConfig");
    assert_eq!(local.directory(&base),base);
    assert_eq!(viewer.directory(&base),base.join("profiles/observer"));
    assert_ne!(viewer.directory(&base),local.directory(&base));
}
#[test]
fn profile_names_cannot_escape_configuration_root_or_select_obsolete_backend() {
    for name in ["..","../other","C:/data","x\\y",""] {
        assert!(Profile::from_args(vec!["--profile".into(),name.into()]).is_err());
    }
    assert!(Profile::from_args(vec!["--simulate".into()]).is_err());
}
