import {createHostClient} from './host-client.js';
import {createDeviceStore} from './device-store.js';
import {mountConsole} from './console-ui.js';
export {connectLocalHost} from './host-client.js';
export function createConsoleSession(client,onChange=()=>{}){
  const store=createDeviceStore(),remotes=new Map(),catalogs=new Map(),renewing=new Set();let page='overview',hostId=null;
  const clientFor=id=>id===hostId||!id?client:remotes.get(id);
  return {store,client,clientFor,addRemote(id,remote){if(id===hostId)throw new Error('This is the local Host');remotes.set(id,remote);},
    removeRemote(id){remotes.delete(id);catalogs.delete(id);},setCatalog(id,value){catalogs.set(id,structuredClone(value));},catalogFor:id=>catalogs.get(id),
    clients:()=>[{hostId,client},...[...remotes].map(([hostId,client])=>({hostId,client}))],get page(){return page;},get hostId(){return hostId;},
    navigate(value){page=value;onChange();},
    apply(event,remote=false){if(!remote)hostId=event.host_id;const result=store.apply(event);store.setLocation(event.host_id,remote);onChange();return result;},
    async heartbeat(){const jobs=[];for(const device of store.all()){
      const key=`${device.hostId}/${device.domain.kind}/${device.domain.id}`,lease=store.lease(key);
      if(lease&&!renewing.has(key)){renewing.add(key);const context=store.get(key)?.context;jobs.push((async()=>{
        try{store.renewLease(key,lease,context,await clientFor(device.hostId).renew(lease.token));}catch{if(store.lease(key)?.token===lease.token)store.dropLease(key);}
        finally{renewing.delete(key);onChange();}
      })());}
    }await Promise.all(jobs);},
    offline(id=hostId){if(id)store.disconnected(id);onChange();},
  };
}
async function start(){
  const native=globalThis.__TAURI__;const content=document.querySelector('#content');
  if(!native?.core?.invoke||!native?.event?.listen){content.textContent='Open Yang LAB INSTRUMENT CONSOLE in the desktop app. No hardware connection is available in this preview.';return;}
  const client=createHostClient(native.core.invoke,native.event.listen);let ui;
  const session=createConsoleSession(client,()=>ui?.render());ui=mountConsole(session,native);
}
if(typeof document!=='undefined')start().catch(error=>{document.querySelector('#notice').textContent=error.message;document.querySelector('#notice').hidden=false;});
