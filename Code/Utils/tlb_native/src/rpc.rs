//! Private finite JSON-lines bridge. No raw SCPI, path, arbitrary program or reset method.
use crate::{Action,Bus,Wire,Result,fail,wavelength_range};
use serde::Deserialize;
use serde_json::{Value,json};
#[derive(Debug,Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {pub id:u64,pub operation:Operation}
#[derive(Debug,Deserialize)]
#[serde(tag="method",rename_all="snake_case",deny_unknown_fields)]
pub enum Operation {Hello,Enumerate,Discover,Connect{key:String},Status{key:String},Action{key:String,action:Action,confirm:bool},Disconnect{key:String},Resources,Shutdown}
pub fn parse(bytes:&[u8])->Result<Request> {
    if bytes.len()>4096 {return Err(fail("protocol","Native request exceeds capacity"));}
    let value:Value=serde_json::from_slice(bytes).map_err(|_|fail("protocol","Invalid native driver request"))?;
    let op=value["operation"].as_object().ok_or_else(||fail("protocol","Missing native operation"))?;
    let expected:&[&str]=match value["operation"]["method"].as_str() {
        Some("hello"|"enumerate"|"discover"|"resources"|"shutdown")=>&["method"],
        Some("connect"|"status"|"disconnect")=>&["method","key"],
        Some("action")=>&["method","key","action","confirm"],_=>return Err(fail("protocol","Unsupported native operation")),
    };
    if op.len()!=expected.len() || expected.iter().any(|k|!op.contains_key(*k)) {
        return Err(fail("protocol","Unknown native operation fields"));
    }
    serde_json::from_slice(bytes).map_err(|_|fail("protocol","Invalid native driver request"))
}
pub fn execute<T:Wire>(bus:&mut Bus<T>,request:Request)->Value {
    let result:Result<Value>=(||Ok(match request.operation {
        Operation::Hello=>json!({"backend":"rust","protocol":1,"pid":std::process::id()}),
        Operation::Resources=>json!({"resources_released":bus.resources_released()}),
        Operation::Enumerate=>json!({"controller_keys":bus.enumerate()?}),
        Operation::Discover=>json!({"identities":bus.discover()?}),
        Operation::Connect{key}=>{let identity=bus.connect(&key)?;json!({"wavelength_range_nm":wavelength_range(&identity.head_model),"identity":identity})},
        Operation::Status{key}=>serde_json::to_value(bus.status(&key)?).unwrap(),
        Operation::Action{key,action,confirm}=>{bus.action(&key,action,confirm)?;json!({"completed":true})},
        Operation::Disconnect{key}=>{bus.disconnect(&key)?;json!({"release_confirmed":true})},
        Operation::Shutdown=>{bus.close_all()?;json!({"release_confirmed":bus.resources_released()})},
    }))();
    match result {Ok(value)=>json!({"v":1,"id":request.id,"ok":true,"result":value,"resources_released":bus.resources_released()}),
        Err(error)=>json!({"v":1,"id":request.id,"ok":false,"error":error,"resources_released":bus.resources_released()})}
}
