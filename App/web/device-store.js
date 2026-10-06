import {deviceKey} from './routes.js';
const copy=value=>structuredClone(value);
const validContext=context=>context&&/^[0-9a-f]{32}$/.test(context.session_id)&&Number.isSafeInteger(context.epoch)&&context.epoch>=0;
export function createDeviceStore(clock=()=>performance.now()){
  const hosts=new Map(),devices=new Map(),histories=new Map(),leases=new Map(),clocks=new Map(),operations=new Map();
  function invalidate(hostId){for(const key of leases.keys())if(key.startsWith(hostId+'/'))leases.delete(key);}
  function archive(key,event){const items=histories.get(key)||[];items.push(copy(event));if(items.length>256)items.shift();histories.set(key,items);}
  function remember(hostId,bootId,record,event){if(!record?.operation_id||!record.domain)return;const key=deviceKey(hostId,record.domain),id=hostId+'/'+bootId+'/'+record.operation_id;
    const old=operations.get(id);if((old?.record.status==='Terminal'&&record.status==='Accepted')||(old?.record.status===record.status&&old.record.phase===record.phase))return;
    operations.delete(id);operations.set(id,{key,bootId,record:copy(record)});while(operations.size>256)operations.delete(operations.keys().next().value);
    archive(key,{...event,type:'operation',domain:record.domain,data:record});}
  function owns(host,domain,lease){const control=host?.control?.[domain.kind+':'+domain.id];return Boolean(host?.connected&&host.synced&&lease?.boot_id===host.bootId&&
    control?.state==='CONTROLLED'&&control.controller_session===lease.session_id&&control.control_epoch===lease.control_epoch);}
  function reconcile(hostId){for(const [key,lease]of leases)if(key.startsWith(hostId+'/')){const device=devices.get(key);if(!device||!owns(hosts.get(hostId),device.domain,lease))leases.delete(key);}}
  function removeAbsent(hostId,data){const present=new Set(Object.keys(data.domains||{}).map(d=>hostId+'/'+d.replace(':','/')));
    for(const key of devices.keys())if(key.startsWith(hostId+'/')&&!present.has(key)){devices.delete(key);leases.delete(key);}}
  function put(hostId,domain,data,event){
    const key=deviceKey(hostId,domain),old=devices.get(key),context=data.context;
    if(context&&(!validContext(context)||context.domain?.id!==domain.id||context.domain?.kind!==domain.kind))return false;
    if(old?.context&&context&&old.context.session_id===context.session_id&&
      (context.epoch<old.context.epoch||(context.epoch===old.context.epoch&&old.context.connection_id&&context.connection_id!==old.context.connection_id))){archive(key,event);return false;}
    devices.set(key,{...copy(data),hostId,domain:copy(domain),receivedAt:clock()});return true;
  }
  return Object.freeze({
    apply(event){
      if(!event||!Number.isSafeInteger(event.seq)||event.seq<0||!event.host_id||!event.boot_id)return {currentChanged:false,requiresSnapshot:true};
      let host=hosts.get(event.host_id);
      if(event.type==='snapshot'){
        if(host?.bootId===event.boot_id&&event.seq<host.seq)return {currentChanged:false,requiresSnapshot:false};
        if(host?.bootId!==event.boot_id){invalidate(event.host_id);for(const key of devices.keys())if(key.startsWith(event.host_id+'/'))devices.delete(key);for(const [id,item]of operations)if(item.key.startsWith(event.host_id+'/'))operations.delete(id);}
        host={...copy(event.data),bootId:event.boot_id,seq:event.seq,synced:true,connected:true,receivedAt:clock()};hosts.set(event.host_id,host);
        removeAbsent(event.host_id,event.data);
        for(const [domainKey,data] of Object.entries(event.data.domains||{})){const [kind,id]=domainKey.split(':');put(event.host_id,{kind,id},data,event);}
        reconcile(event.host_id);
        for(const record of [...(event.data.operations||[]),...(event.data.results||[])])remember(event.host_id,event.boot_id,record,event);
        return {currentChanged:true,requiresSnapshot:false};
      }
      const first=event.first_seq??event.seq;
      if(host?.bootId===event.boot_id&&event.seq<=host.seq)return {currentChanged:false,requiresSnapshot:false};
      if(!host||host.bootId!==event.boot_id||!Number.isSafeInteger(first)||first>event.seq||first!==host.seq+1){if(host)host.synced=false;invalidate(event.host_id);return {currentChanged:false,requiresSnapshot:true};}
      host.seq=event.seq;host.receivedAt=clock();
      if(event.type==='domain')return {currentChanged:put(event.host_id,event.domain,event.data,event),requiresSnapshot:false};
      if(event.type==='state'){
        Object.assign(host,copy(event.data));
        removeAbsent(event.host_id,event.data);
        let changed=false;for(const [domainKey,data] of Object.entries(event.data.domains||{})){const [kind,id]=domainKey.split(':');changed=put(event.host_id,{kind,id},data,event)||changed;}
        reconcile(event.host_id);
        for(const record of [...(event.data.operations||[]),...(event.data.results||[])])remember(event.host_id,event.boot_id,record,event);
        return {currentChanged:changed,requiresSnapshot:false};
      }
      if(event.type==='operation'){remember(event.host_id,event.boot_id,event.data,event);return {currentChanged:false,requiresSnapshot:false};}
      return {currentChanged:false,requiresSnapshot:false};
    },
    get:key=>devices.has(key)?copy(devices.get(key)):null,
    host:id=>hosts.has(id)?copy(hosts.get(id)):null,
    hosts:()=>[...hosts.values()].map(copy),
    setLocation(id,remote){const host=hosts.get(id);if(host)host.remote=remote;},
    all:()=>[...devices.values()].map(copy),
    history:key=>copy(histories.get(key)||[]),
    operationRecords:key=>[...operations.values()].filter(item=>item.key===key).map(copy),
    setLease(key,lease){if(!devices.has(key))throw new Error('Unknown instance');leases.set(key,{...copy(lease),receivedAt:clock()});},
    lease:key=>leases.has(key)?copy(leases.get(key)):null,
    renewLease(key,basis,context,next){const current=leases.get(key),device=devices.get(key),host=hosts.get(device?.hostId);
      if(!current||current.token!==basis.token||current.control_epoch!==basis.control_epoch||current.session_id!==basis.session_id||
        next.token!==basis.token||next.boot_id!==basis.boot_id||next.control_epoch!==basis.control_epoch||next.session_id!==basis.session_id||
        JSON.stringify(device?.context)!==JSON.stringify(context)||!owns(host,device.domain,basis))return false;
      leases.set(key,{...copy(next),receivedAt:clock()});return true;},
    dropLease:key=>leases.delete(key),
    disconnected(hostId){const host=hosts.get(hostId);if(host){host.connected=false;host.synced=false;}invalidate(hostId);
      for(const [key,device]of devices)if(key.startsWith(hostId+'/'))devices.set(key,{...device,communication:'UNKNOWN',freshness:'Freshness Unknown'});},
    canControl(key){const device=devices.get(key),host=hosts.get(device?.hostId),lease=leases.get(key);
      return Boolean(device?.context&&owns(host,device.domain,lease)&&clock()-lease.receivedAt<lease.expires_in_ms);
    },
    setClock(hostId,serverMs,start,end){if(![serverMs,start,end].every(Number.isFinite)||end<start)return;
      clocks.set(hostId,{low:serverMs-end,high:serverMs-start,at:end});},
    ageUpperMs(hostId,sampleHostMs){const calibration=clocks.get(hostId),now=clock();
      if(!calibration||now<calibration.at||now-calibration.at>60000||!Number.isFinite(sampleHostMs))return null;
      return Math.max(0,now+calibration.high-sampleHostMs);
    },
  });
}
