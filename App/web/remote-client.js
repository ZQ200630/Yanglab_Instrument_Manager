import {createHostClient} from './host-client.js';

/** Native TLS owns pairing secrets. This adapter supplies only the owning Host key. */
export function createRemoteClient(invoke,listen,hostId){
  if(!/^[0-9a-f]{32}$/.test(hostId))throw new Error('Invalid remote Host identity');
  let rpc=0;const namespace=crypto.randomUUID().replaceAll('-','');
  async function result(method,params){
    const request={v:1,id:`remote-${namespace}-${++rpc}`,method,params};
    const reply=await invoke('remote_call',{hostId,request});
    if(reply?.v!==1||reply.id!==request.id||typeof reply.ok!=='boolean')throw Object.assign(new Error('Remote reply mismatch'),{outcomeUnknown:true});
    if(!reply.ok)throw Object.assign(new Error(reply.error?.message||'Remote request failed'),{code:reply.error?.code});
    return reply.result;
  }
  const bridge=async(command,args)=>{
    switch(command){
      case 'host_connect':return invoke('remote_connect',{hostId});
      case 'host_disconnect':return invoke('remote_disconnect',{hostId});
      case 'host_call':return invoke('remote_call',{hostId,request:args.request});
      case 'host_heartbeat':return result('renew_control',{token:args.token});
      case 'host_result':return result('read_result',args.params);
      case 'host_subscribe':return invoke('remote_subscribe',{hostId});
      case 'export_archive':return invoke('remote_export',{reference:args.reference});
      default:throw new Error('Configure the owning Host on its local computer');
    }
  };
  return createHostClient(bridge,async(name,callback)=>{
    if(name!=='host-event')throw new Error('Unsupported remote subscription');
    return listen('remote-event',event=>{const envelope=event.payload;if(envelope?.host_id===hostId&&envelope.event?.host_id===hostId)callback({payload:envelope.event});});
  });
}
