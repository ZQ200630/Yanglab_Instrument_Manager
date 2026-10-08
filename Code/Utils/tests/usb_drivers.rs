use yang_drivers::usb_drivers::*;
fn record(id: &str, service: &str, code: Option<u32>) -> UsbDeviceRecord {
    UsbDeviceRecord { instance_id: id.into(), description: "USB device".into(), service: service.into(), problem_code: code }
}
#[test]
fn absent_devices_are_not_installable_and_malformed_ids_are_ignored() {
    let ids = ["ROOT\\USB\\VID_1A86&PID_7523\\a", "USB\\VID_1A86&PID_75230\\a", "USB\\VID_1A86&PID_7523x", "USB\\VID_9999&PID_7523\\a"];
    let records: Vec<_> = ids.iter().map(|id| record(id, "", Some(28))).collect();
    let result = serial_usb_from_records(&records).unwrap();
    assert_eq!(result.ch340.state, DriverState::NotDetected);
    assert_eq!(result.cp210x.state, DriverState::NotDetected);
    assert!(result.ch340.devices.is_empty());
}
#[test]
fn supported_ids_services_and_unknown_status_are_classified_without_com_ports() {
    for pid in ["7523", "5523"] {
        for service in ["CH341SER", "ch341ser_a", "CH341SER_A64", "CH341SER_M64"] {
            let id = format!("usb\\vid_1a86&pid_{pid}\\finite");
            for (code, want) in [(Some(0), DriverState::Ready), (Some(28), DriverState::Missing), (Some(10), DriverState::Unavailable), (Some(22), DriverState::Unavailable), (None, DriverState::Unavailable)] {
                let result = serial_usb_from_records(&[record(&id, service, code)]).unwrap();
                assert_eq!(result.ch340.state, want);
                let value = serde_json::to_value(&result.ch340.devices[0]).unwrap();
                assert_eq!(value["instance_id"], id);
                assert_eq!(value["service"], service);
                assert_eq!(value["problem_code"], serde_json::to_value(code).unwrap());
                assert!(value.get("record").is_none());
                assert!(value.get("resource").is_none());
            }
        }
    }
    for service in ["", "USBCCGP", "WINUSB"] {
        assert_eq!(serial_usb_from_records(&[record("USB\\VID_1A86&PID_7523\\a", service, Some(0))]).unwrap().ch340.state, DriverState::Unavailable);
    }
    for pid in ["EA60", "EA63", "EA70", "EA71", "EA7A", "EA7B"] {
        let id = format!("USB\\VID_10C4&PID_{pid}&MI_00\\a");
        assert_eq!(serial_usb_from_records(&[record(&id, "silabser", Some(0))]).unwrap().cp210x.state, DriverState::Ready);
    }
}
#[test]
fn composite_parents_do_not_hide_interface_faults_and_families_remain_independent() {
    let mut records = vec![];
    for pid in ["EA70", "EA71", "EA7A", "EA7B"] {
        for suffix in ["\\parent", "&MI_0\\a", "&MI_000\\a", "&MI_G0\\a", "&REV_0001&MI_00\\a"] {
            records.push(record(&format!("USB\\VID_10C4&PID_{pid}{suffix}"), "USBCCGP", Some(28)));
        }
    }
    assert_eq!(serial_usb_from_records(&records).unwrap().cp210x.state, DriverState::NotDetected);
    records.push(record("USB\\VID_10C4&PID_EA70&MI_00\\a", "SILABSER", Some(0)));
    records.push(record("USB\\VID_10C4&PID_EA70&MI_01\\a", "SILABSER", None));
    assert_eq!(serial_usb_from_records(&records).unwrap().cp210x.state, DriverState::Unavailable);
    records.push(record("USB\\VID_10C4&PID_EA70&MI_02\\a", "", Some(28)));
    records.push(record("USB\\VID_1A86&PID_5523\\a", "CH341SER", Some(0)));
    let result = serial_usb_from_records(&records).unwrap();
    assert_eq!(result.cp210x.state, DriverState::Missing);
    assert_eq!(result.cp210x.devices.len(), 3);
    assert_eq!(result.ch340.state, DriverState::Ready);
}
#[test]
fn newport_keeps_device_and_sdk_states_separate() {
    let records = [record("USB\\VID_104D&PID_100A\\a", "winusb", Some(0)), record("USB\\VID_1A86&PID_7523\\b", "", Some(28))];
    let result = newport_from_records(&records, SdkStatus::unavailable("incompatible")).unwrap();
    let value = serde_json::to_value(result).unwrap();
    assert_eq!(value["devices"].as_array().unwrap().len(), 1);
    assert_eq!(value["devices"][0]["driver_state"], "ready");
    assert_eq!(value["sdk"]["state"], "unavailable");
    assert!(value["sdk"].get("path").is_none());
    assert!(value.get("state").is_none());
    assert_eq!(value["download_url"], "https://download.newport.com/#/Software/Newport_USB_Driver/");
    for (code, service, want) in [(Some(28), "", DriverState::Missing), (None, "WINUSB", DriverState::Unavailable), (Some(0), "other", DriverState::Unavailable)] {
        assert_eq!(newport_from_records(&[record("USB\\VID_104D&PID_100A\\a", service, code)], SdkStatus::unavailable("x")).unwrap().devices[0].driver_state, want);
    }
}
#[test]
fn unbounded_records_or_strings_fail_closed() {
    let row = record("USB\\VID_1A86&PID_7523\\a", "CH341SER", Some(0));
    assert!(serial_usb_from_records(&vec![row.clone(); 4097]).is_err());
    let mut long = row;
    long.instance_id = "a".repeat(512);
    assert!(serial_usb_from_records(&[long]).is_err());
}
