use yang_lab_tlb::{Bus,rpc::{self,Operation},sdk::{Sdk,OwnerGuard}};
use serde_json::json;
use std::io::{BufRead,Write};
fn stdio(bus:&mut Bus<Sdk>)->Result<(),String> {
    let stdin=std::io::stdin();let mut input=stdin.lock();let stdout=std::io::stdout();let mut output=stdout.lock();
    loop {
        let mut bytes=Vec::new();let size=std::io::Read::take(&mut input,4098).read_until(b'\n',&mut bytes).map_err(|e|e.to_string())?;
        if size==0 {break;}
        if size>4097 || bytes.last()!=Some(&b'\n') {return Err("Invalid bounded native frame".into());}
        let request=rpc::parse(&bytes).map_err(|e|e.message)?;
        let shutdown=matches!(&request.operation,Operation::Shutdown);
        let reply=rpc::execute(bus,request);let released=reply["resources_released"]==true;
        let wrote=serde_json::to_writer(&mut output,&reply).and_then(|_|{output.write_all(b"\n").map_err(serde_json::Error::io)?;output.flush().map_err(serde_json::Error::io)});
        if let Err(e)=wrote {return Err(e.to_string());}
        if shutdown && released {break;}
    }
    Ok(())
}
fn main(){
    let result=(||->Result<(),String>{
        let mut args=std::env::args().skip(1);let mode=args.next().ok_or("Use --stdio, --enumerate or --read-only <controller key>")?;
        let key=if mode=="--read-only"{Some(args.next().ok_or("Controller key is required")?)}else{None};
        if args.next().is_some() || !matches!(mode.as_str(),"--stdio"|"--enumerate"|"--read-only") {return Err("Invalid native diagnostic arguments".into());}
        let _owner=OwnerGuard::acquire().map_err(|e|e.message)?;
        let mut bus=Bus::new(Sdk::default());
        let work=match mode.as_str(){"--stdio"=>stdio(&mut bus),"--enumerate"=>bus.enumerate().map(|keys|println!("{}",json!({"backend":"rust","controller_keys":keys,"release_confirmed":bus.resources_released()}))).map_err(|e|e.message),
            _=>{let key=key.unwrap();( || {let identity=bus.connect(&key).map_err(|e|e.message)?;let status=bus.status(&key).map_err(|e|e.message)?;bus.disconnect(&key).map_err(|e|e.message)?;
                println!("{}",json!({"backend":"rust","identity":identity,"readback":status,"release_confirmed":bus.resources_released()}));Ok(())})()}};
        // Normal EOF/error cleanup preserves output and settings; never kill an owner or command zero.
        if let Err(e)=bus.close_all(){eprintln!("Native cleanup retained: {}",e.message);loop{std::thread::park();}}
        work
    })();
    if let Err(error)=result {eprintln!("{error}");std::process::exit(1);}
}
