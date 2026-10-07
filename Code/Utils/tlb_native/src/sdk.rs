//! Newport 5.x Windows x64 C ABI. Every call stays on the owner thread.
use crate::{fail,Result,Wire,controller_identity};
use std::{collections::BTreeMap,ffi::c_void,path::{Path,PathBuf},time::Duration};

pub trait PacketIo {
    fn pace(&mut self,_command:&str,_retry:bool) {}
    fn send(&mut self,index:i32,buffer:&mut [u8;64],length:u32)->Result<()>;
    fn read(&mut self,index:i32)->Result<([u8;64],usize)>;
}
pub fn encode(command:&str)->Result<Vec<u8>> {
    if command.is_empty() || !command.bytes().all(|b|(32..=126).contains(&b)) {
        return Err(fail("protocol","Invalid Newport command framing"));
    }
    let (header,arg)=command.split_once(' ').map_or((command,None),|(h,a)|(h,Some(a)));
    let header=header.split(':').map(|part|{
        let word=part.trim_end_matches('?');
        let full=match word {"SYST"=>"SYSTem","LAS"=>"LASer","OUTP"=>"OUTPut","STAT"=>"STATe",
            "TRAC"=>"TRACk","SOUR"=>"SOURce","WAVE"=>"WAVElength","CPOW"=>"CPOWer",
            "VOLT"=>"VOLTage","PIEZ"=>"PIEZo","CURR"=>"CURRent","SENS"=>"SENSe",
            "POW"=>"POWer","DIODE"=>"DIODe","FORW"=>"FORWard","RET"=>"RETurn",_=>word};
        format!("{full}{}",if part.ends_with('?'){"?"}else{""})
    }).collect::<Vec<_>>().join(":");
    let text=arg.map_or(header.clone(),|a|format!("{header} {a}"));
    if text.len()>=64 {return Err(fail("protocol","Newport command exceeds the 64-byte packet"));}
    Ok(text.into_bytes())
}
pub fn frame(packet:&[u8],count:usize)->Result<String> {
    if count==0 || count>64 || count>packet.len() {return Err(fail("protocol","Invalid Newport reply byte count"));}
    let bytes=&packet[..count];
    let end=bytes.windows(2).position(|w|w==b"\r\n").ok_or_else(||fail("protocol","TLB response is not terminated by CRLF"))?;
    if end==0 || !bytes[..end].iter().all(|b|(32..=126).contains(b)) {return Err(fail("protocol","Invalid TLB scalar frame"));}
    // Firmware 2.4 returns a full packet with stale bytes after the first CRLF.
    Ok(String::from_utf8(bytes[..end].to_vec()).unwrap())
}
pub fn drain(io:&mut impl PacketIo,index:i32)->Result<()> {
    for _ in 0..16 {match io.read(index) {Err(e) if e.kind=="read_failure"=>return Ok(()),Err(e)=>return Err(e),Ok(_)=>{}}}
    Err(fail("protocol","Newport input did not become quiescent"))
}
fn transaction(io:&mut impl PacketIo,index:i32,command:&str,retry:bool)->Result<String> {
    let bytes=encode(command)?;let mut buffer=[0;64];buffer[..bytes.len()].copy_from_slice(&bytes);
    io.pace(command,retry);
    io.send(index,&mut buffer,bytes.len() as u32)?;
    let (packet,count)=io.read(index)?;frame(&packet,count)
}
fn readonly(command:&str)->bool {
    matches!(command,"*IDN?"|"*OPC?"|"*STB?"|"SYST:LAS:MODEL?"|"SYST:LAS:SN?"|
        "OUTP:STAT?"|"OUTP:TRAC?"|"SYST:MCONT?"|"SOUR:CPOW?"|"SENS:WAVE"|"SOUR:WAVE?"|
        "SENS:POW:DIODE"|"SOUR:POW:DIODE?"|"SENS:CURR:DIODE"|"SOUR:CURR:DIODE?"|"SOUR:VOLT:PIEZ?"|
        "SOUR:WAVE:MAXVEL?"|"SOUR:WAVE:START?"|"SOUR:WAVE:STOP?"|"SOUR:WAVE:SLEW:FORW?"|"SOUR:WAVE:SLEW:RET?"|"SOUR:WAVE:DESSCANS?")
}
pub fn query(io:&mut impl PacketIo,index:i32,command:&str)->Result<String> {
    let first=transaction(io,index,command,false);
    let retry=readonly(command) && match &first {Err(e)=>e.kind=="read_failure",Ok(v)=>matches!(v.as_str(),"COMMAND NOT VALID"|"NO PARAMETER SPECIFIED")};
    if retry {drain(io,index)?;transaction(io,index,command,true)} else {first}
}
// Qualify the faster read-only path separately. Sent setters are never replayed.
pub fn pacing_ms(command:&str,retry:bool)->u64 {if readonly(command)&&!retry {10} else {200}}
fn validate_pe(path:&Path)->Result<()> {
    use std::io::{Read,Seek,SeekFrom};
    let mut f=std::fs::File::open(path).map_err(|e|fail("connection",e.to_string()))?;
    let mut header=[0;64];f.read_exact(&mut header).map_err(|e|fail("connection",e.to_string()))?;
    let offset=u32::from_le_bytes(header[60..64].try_into().unwrap());
    if &header[..2]!=b"MZ" || offset>16*1024*1024 {return Err(fail("connection","Invalid Newport DLL PE header"));}
    f.seek(SeekFrom::Start(offset as u64)).map_err(|e|fail("connection",e.to_string()))?;
    let mut pe=[0;6];f.read_exact(&mut pe).map_err(|e|fail("connection",e.to_string()))?;
    if pe!=[b'P',b'E',0,0,0x64,0x86] {return Err(fail("connection","Newport DLL must be Windows x64"));}Ok(())
}
pub fn dll_path()->Result<PathBuf> {
    for variable in ["ProgramW6432","ProgramFiles","ProgramFiles(x86)"] {
        if let Some(root)=std::env::var_os(variable) {
            let bin=PathBuf::from(root).join("Newport/Newport USB Driver/Bin");
            for path in [bin.join("usbdll.dll"),bin.join("x64/usbdll.dll")] {
                if path.is_file() && validate_pe(&path).is_ok() {return path.canonicalize().map_err(|e|fail("connection",e.to_string()));}
            }
        }
    }
    Err(fail("connection","Newport USB SDK is missing. Install Newport USB Driver"))
}
#[cfg(windows)]
mod windows {
    use super::*;
    use windows_sys::Win32::{Foundation::{HMODULE,FreeLibrary,HANDLE,CloseHandle,GetLastError,ERROR_ALREADY_EXISTS},
        System::{LibraryLoader::{LoadLibraryExW,GetProcAddress,LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,LOAD_LIBRARY_SEARCH_DEFAULT_DIRS},Threading::CreateMutexW}};
    use std::os::windows::ffi::OsStrExt;
    type Open=unsafe extern "system" fn(i32,bool,*mut i32)->i32;
    type Info=unsafe extern "system" fn(*mut c_void)->i32;
    type Send=unsafe extern "system" fn(i32,*mut c_void,u32)->i32;
    type Read=unsafe extern "system" fn(i32,*mut c_void,u32,*mut u32)->i32;
    type Close=unsafe extern "system" fn();
    struct Api {library:HMODULE,open:Open,info:Info,send:Send,read:Read,close:Close}
    impl Drop for Api {fn drop(&mut self){unsafe{FreeLibrary(self.library);}}}
    impl Api {
        fn load()->Result<Self> {
            let path=dll_path()?;let wide:Vec<u16>=path.as_os_str().encode_wide().chain(Some(0)).collect();
            // Absolute architecture-checked vendor file; dependency search stays in its directory and system defaults.
            let library=unsafe{LoadLibraryExW(wide.as_ptr(),std::ptr::null_mut(),LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR|LOAD_LIBRARY_SEARCH_DEFAULT_DIRS)};
            if library.is_null(){return Err(fail("connection",format!("Could not load Newport SDK: {}",std::io::Error::last_os_error())));}
            unsafe fn symbol<T:Copy>(library:HMODULE,name:&[u8])->Result<T> {
                let address=GetProcAddress(library,name.as_ptr()).ok_or_else(||fail("connection","Missing required Newport SDK export"))?;
                // Each caller supplies the exact Newport 5.x prototype; Windows x64 function pointers share size/ABI.
                Ok(std::mem::transmute_copy(&address))
            }
            let result=(||unsafe{Ok(Self{library,open:symbol(library,b"newp_usb_open_devices\0")?,info:symbol(library,b"newp_usb_get_device_info\0")?,
                send:symbol(library,b"newp_usb_send_ascii\0")?,read:symbol(library,b"newp_usb_get_ascii\0")?,close:symbol(library,b"newp_usb_uninit_system\0")?})})();
            if result.is_err(){unsafe{FreeLibrary(library);}}result
        }
    }
    /// Cross-process ownership fence also survives loss of the Python parent while a native call is blocked.
    pub struct OwnerGuard(HANDLE);
    impl OwnerGuard {
        pub fn acquire()->Result<Self> {
            let name:Vec<u16>="Local\\YangLab-Newport-TLB6700".encode_utf16().chain(Some(0)).collect();
            let handle=unsafe{CreateMutexW(std::ptr::null(),0,name.as_ptr())};
            if handle.is_null(){return Err(fail("connection","Could not establish native Newport ownership"));}
            if unsafe{GetLastError()}==ERROR_ALREADY_EXISTS {unsafe{CloseHandle(handle);}return Err(fail("connection","Another native Newport owner is running; disconnect it first"));}
            Ok(Self(handle))
        }
    }
    impl Drop for OwnerGuard {fn drop(&mut self){unsafe{CloseHandle(self.0);}}}
    pub struct Sdk {api:Option<Api>,ids:BTreeMap<String,i32>}
    impl Default for Sdk {fn default()->Self{Self{api:None,ids:BTreeMap::new()}}}
    fn checked(code:i32,operation:&str)->Result<()> {
        if code!=0 {Err(fail(if operation=="read" && code==-1{"read_failure"}else{"connection"},format!("Newport SDK {operation} failed ({code}); check USB connection and other applications")))}else{Ok(())}
    }
    impl PacketIo for Sdk {
        fn pace(&mut self,command:&str,retry:bool) {std::thread::sleep(Duration::from_millis(pacing_ms(command,retry)));}
        fn send(&mut self,index:i32,buffer:&mut [u8;64],length:u32)->Result<()> {
            let api=self.api.as_ref().ok_or_else(||fail("connection","SDK is closed"))?;
            checked(unsafe{(api.send)(index,buffer.as_mut_ptr().cast(),length)},"write")
        }
        fn read(&mut self,index:i32)->Result<([u8;64],usize)> {
            let api=self.api.as_ref().ok_or_else(||fail("connection","SDK is closed"))?;
            let mut packet=[0;64];let mut count=0;
            checked(unsafe{(api.read)(index,packet.as_mut_ptr().cast(),64,&mut count)},"read")?;
            Ok((packet,count as usize))
        }
    }
    impl Wire for Sdk {
        fn open(&mut self)->Result<Vec<String>> {
            if self.api.is_some(){return Err(fail("connection","SDK lifetime is already open"));}
            self.api=Some(Api::load()?);let api=self.api.as_ref().unwrap();let mut count=0;
            checked(unsafe{(api.open)(0x100A,false,&mut count)},"open")?;
            if !(0..=32).contains(&count){return Err(fail("protocol","Invalid Newport device count"));}
            let mut info=[0u8;8192];checked(unsafe{(api.info)(info.as_mut_ptr().cast())},"identity enumeration")?;
            self.ids.clear();
            for index in 0..count {
                drain(self,index)?;let identity=query(self,index,"*IDN?")?;
                let (serial,_)=controller_identity(&identity)?;let key=format!("6700 SN{serial}");
                if self.ids.insert(key,index).is_some(){return Err(fail("protocol","Duplicate Newport controller enumeration"));}
            }
            Ok(self.ids.keys().cloned().collect())
        }
        fn query(&mut self,key:&str,command:&str)->Result<String> {
            let index=*self.ids.get(key).ok_or_else(||fail("connection","Unknown controller serial"))?;query(self,index,command)
        }
        fn close(&mut self)->Result<()> {
            if let Some(api)=self.api.take(){unsafe{(api.close)();}drop(api);}
            self.ids.clear();Ok(())
        }
    }
}
#[cfg(windows)] pub use windows::{Sdk,OwnerGuard};
