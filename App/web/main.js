import {createHostClient} from './host-client.js';
import {createDeviceStore} from './device-store.js';
import {mountConsole} from './console-ui.js';
export {connectLocalHost} from './host-client.js';
export function createConsoleSession(client,onChange=()=>{}){
  const store=createDeviceStore();let page='overview',hostId=null,renewing=false;
  return {store,client,get page(){return page;},get hostId(){return hostId;},
    navigate(value){page=value;onChange();},
    apply(event){hostId=event.host_id;const result=store.apply(event);onChange();return result;},
    async heartbeat(){if(renewing)return;renewing=true;let changed=false;try{for(const device of store.all()){
      const key=`${device.hostId}/${device.domain.kind}/${device.domain.id}`,lease=store.lease(key);
      if(lease){changed=true;const context=store.get(key)?.context;try{store.renewLease(key,lease,context,await client.renew(lease.token));}catch{if(store.lease(key)?.token===lease.token)store.dropLease(key);}}
    }}finally{renewing=false;if(changed)onChange();}},
    offline(){if(hostId)store.disconnected(hostId);onChange();},
  };
}
async function start(){
  const native=globalThis.__TAURI__;const content=document.querySelector('#content');
  if(!native?.core?.invoke||!native?.event?.listen){content.textContent='Open Yang LAB INSTRUMENT CONSOLE in the desktop app. No hardware connection is available in this preview.';return;}
  const client=createHostClient(native.core.invoke,native.event.listen);let ui;
  const session=createConsoleSession(client,()=>ui?.requestRender());ui=mountConsole(session,native);
}
if(typeof document!=='undefined')start().catch(error=>{document.querySelector('#notice').textContent=error.message;document.querySelector('#notice').hidden=false;});
