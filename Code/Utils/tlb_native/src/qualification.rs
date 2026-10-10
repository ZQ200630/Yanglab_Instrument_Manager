//! Reviewed compile-time RESET qualifications, never an operator override.
use crate::Identity;
use serde::Deserialize;
use std::collections::BTreeSet;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest { schema:u32, records:Vec<Profile> }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    identity:Identity,
    sdk_sha256:String,
    protocol_revision:u32,
    rates_nm_s:Vec<f64>,
    evidence_sha256:Vec<String>,
}
fn hash(value:&str)->bool {
    value.len()==64 && value.bytes().all(|c|c.is_ascii_digit()||(b'a'..=b'f').contains(&c))
}
fn bounded_text(value:&str)->bool {
    !value.is_empty() && value.len()<=64 && value.trim()==value && value.bytes().all(|c|(32..=126).contains(&c))
}

pub fn reviewed_rates(identity: &Identity, sdk_sha256: &str) -> Vec<f64> {
    rates_from(include_str!("reviewed_single_scans.json"), identity, sdk_sha256)
}

fn rates_from(document: &str, identity: &Identity, sdk_sha256: &str) -> Vec<f64> {
    if document.len()>65536 || !hash(sdk_sha256) {return Vec::new();}
    let Ok(manifest)=serde_json::from_str::<Manifest>(document) else {return Vec::new();};
    if manifest.schema!=1 || manifest.records.len()>32 {return Vec::new();}
    let mut identities=BTreeSet::new();
    for p in &manifest.records {
        let id=&p.identity;
        let Some((_,_,head_max))=crate::model_spec(&id.head_model) else {return Vec::new();};
        if id.manufacturer!="New Focus" || id.model!="TLB-6700" ||
            !crate::valid_key(&format!("6700 SN{}",id.serial)) ||
            !bounded_text(&id.firmware) || !bounded_text(&id.head_serial) ||
            !hash(&p.sdk_sha256) || p.protocol_revision!=1 || p.rates_nm_s!=[0.05,0.1] ||
            p.rates_nm_s.iter().any(|v|!v.is_finite()||*v>head_max) ||
            p.evidence_sha256.len()!=6 || p.evidence_sha256.iter().any(|v|!hash(v)) ||
            p.evidence_sha256.iter().collect::<BTreeSet<_>>().len()!=6 ||
            !identities.insert((id.manufacturer.clone(),id.model.clone(),id.serial.clone(),
                id.firmware.clone(),id.head_model.clone(),id.head_serial.clone(),p.sdk_sha256.clone())) {
            return Vec::new();
        }
    }
    manifest.records.into_iter().find(|p|p.identity==*identity&&p.sdk_sha256==sdk_sha256)
        .map(|p|p.rates_nm_s).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn identity() -> Identity {
        Identity { manufacturer:"New Focus".into(), model:"TLB-6700".into(), serial:"1012".into(),
            firmware:"2.4".into(), head_model:"6722-P".into(), head_serial:"P1001".into() }
    }
    fn manifest() -> Value {
        json!({"schema":1,"records":[{"identity":identity(),"sdk_sha256":"a".repeat(64),
            "protocol_revision":1,"rates_nm_s":[0.05,0.1],
            "evidence_sha256":(1..=6).map(|v|format!("{v:064x}")).collect::<Vec<_>>()}]})
    }
    fn rates(document: &Value, id: &Identity, sdk: &str) -> Vec<f64> {
        rates_from(&document.to_string(), id, sdk)
    }
    #[test]
    fn compiled_empty_table_never_claims_real_support() {
        assert!(reviewed_rates(&identity(), &"a".repeat(64)).is_empty());
    }
    #[test]
    fn reviewed_complete_record_matches_exact_identity_and_sdk_only() {
        let document=manifest(); let id=identity(); let sdk="a".repeat(64);
        assert_eq!(rates(&document,&id,&sdk),[0.05,0.1]);
        for field in ["manufacturer","model","serial","firmware","head_model","head_serial"] {
            let mut changed=serde_json::to_value(&id).unwrap(); changed[field]=json!("changed");
            assert!(rates(&document,&serde_json::from_value(changed).unwrap(),&sdk).is_empty());
        }
        assert!(rates(&document,&id,&"b".repeat(64)).is_empty());
    }
    #[test]
    fn multiple_distinct_reviewed_controllers_do_not_steal_qualification() {
        let mut document=manifest(); let mut second=document["records"][0].clone();
        second["identity"]["serial"]=json!("1013"); second["identity"]["head_serial"]=json!("P1002");
        document["records"].as_array_mut().unwrap().push(second.clone());
        let id:Identity=serde_json::from_value(second["identity"].clone()).unwrap();
        assert_eq!(rates(&document,&id,&"a".repeat(64)),[0.05,0.1]);
        assert_eq!(rates(&document,&identity(),&"a".repeat(64)),[0.05,0.1]);
    }
    #[test]
    fn invalid_records_fail_closed_instead_of_skipping_unreviewed_parts() {
        for (path,value) in [
            (vec!["schema"],json!(2)),
            (vec!["records","0","protocol_revision"],json!(2)),
            (vec!["records","0","sdk_sha256"],json!("A".repeat(64))),
            (vec!["records","0","sdk_sha256"],json!("g".repeat(64))),
            (vec!["records","0","rates_nm_s"],json!([0.1])),
            (vec!["records","0","rates_nm_s"],json!([0.05,0.2])),
            (vec!["records","0","rates_nm_s"],json!([0.1,0.05])),
            (vec!["records","0","evidence_sha256"],json!(vec!["1".repeat(64);6])),
            (vec!["records","0","evidence_sha256"],json!([])),
            (vec!["records","0","identity","head_model"],json!("6722-CUSTOM")),
            (vec!["records","0","identity","head_serial"],json!("")),
            (vec!["records","0","identity","serial"],json!("not-a-controller")),
        ] {
            let mut document=manifest(); let mut target=&mut document;
            for key in path { target=if let Ok(index)=key.parse::<usize>() {&mut target[index]} else {&mut target[key]}; }
            *target=value;
            assert!(rates(&document,&identity(),&"a".repeat(64)).is_empty(),"{document}");
        }
        let mut duplicate=manifest(); let record=duplicate["records"][0].clone();
        duplicate["records"].as_array_mut().unwrap().push(record);
        assert!(rates(&duplicate,&identity(),&"a".repeat(64)).is_empty());
        let mut unknown=manifest(); unknown["override_supported"]=json!(true);
        assert!(rates(&unknown,&identity(),&"a".repeat(64)).is_empty());
        let mut unknown_record=manifest(); unknown_record["records"][0]["skip_verification"]=json!(true);
        assert!(rates(&unknown_record,&identity(),&"a".repeat(64)).is_empty());
        let too_many=json!({"schema":1,"records":vec![manifest()["records"][0].clone();33]});
        assert!(rates(&too_many,&identity(),&"a".repeat(64)).is_empty());
        assert!(rates_from(&" ".repeat(65537),&identity(),&"a".repeat(64)).is_empty());
        assert!(rates_from("{broken",&identity(),&"a".repeat(64)).is_empty());
    }
}
