#[cfg(test)]
#[path="profile_tests.rs"]
mod tests;

use crate::host::{contracts::HostError,registry::new_id};
use std::path::{Path,PathBuf};
use serde_json::{json,Value};
use tauri::Manager;

#[derive(Default)]
pub struct Profile { pub network_only:bool, name:Option<String> }
impl Profile {
    pub fn from_args(args:impl IntoIterator<Item=String>)->Result<Self,HostError> {
        let mut result=Self::default();let mut args=args.into_iter();
        while let Some(arg)=args.next(){match arg.as_str(){
            "--network-only"=>result.network_only=true,
            "--profile"=>{
                let name=args.next().filter(|s|!s.is_empty()&&s.len()<=40&&s.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'-'||b==b'_'))
                    .ok_or_else(||HostError::new("AppProfile","Use a short letters/digits profile name"))?;
                result.name=Some(name);
            },
            _=>return Err(HostError::new("AppProfile","Unknown App argument")),
        }}
        if result.network_only&&result.name.is_none(){result.name=Some("observer".into());}
        Ok(result)
    }
    pub fn directory(&self,base:&Path)->PathBuf {
        match &self.name {Some(name)=>base.join("profiles").join(name),None=>base.to_owned()}
    }
    pub fn require_hardware_host(&self)->Result<(),HostError> {
        if self.network_only {Err(HostError::new("NetworkOnly","This profile cannot open a local hardware Host"))}else{Ok(())}
    }
}
pub(crate) fn config_dir(app:&tauri::AppHandle)->Result<PathBuf,HostError> {
    let base=app.path().app_config_dir().map_err(|_|HostError::new("AppProfile","App configuration unavailable"))?;
    Ok(app.state::<Profile>().directory(&base))
}
#[tauri::command]
pub async fn app_profile(app:tauri::AppHandle)->Result<Value,HostError> {
    let profile=app.state::<Profile>();
    let host_id=if profile.network_only {
        let path=config_dir(&app)?.join("observer-id.dpapi");
        if path.exists(){crate::remote::load_secret::<String>(&path)?}else{
            let id=new_id()?;crate::remote::save_secret(&path,&id)?;id
        }
    }else{String::new()};
    if profile.network_only&&!crate::host::contracts::valid_id(&host_id){return Err(HostError::new("AppProfile","Invalid observer identity"));}
    Ok(json!({"network_only":profile.network_only,"host_id":host_id,"name":profile.name}))
}
