import {signature,canSave} from './setup.js';import {deviceKey} from './routes.js';
export function createSetupActions(session,confirm,resync,run){
  const client=session.client,store=session.store;const host=()=>store.host(session.hostId),rev=()=>host().registry.registry_rev;
  async function ensure(d){
    if(d.record){if(d.record.mode==='unverified'||d.recordSignature!==signature(d)){
      if(!confirm('Replace this imported or changed draft with a new identity? Any retained session must be released first.'))throw new Error('Draft replacement cancelled');
      await client.cancelDraft({draft_id:d.record.device_id,expected_rev:rev()});store.dropLease(deviceKey(session.hostId,{kind:'device',id:d.record.device_id}));d.record=null;d.proof=null;await resync();
    }else return d.record;}
    d.record=await client.createDraft({model_id:d.modelId,profile_id:d.profileId,params:d.params,name:d.name,expected_rev:rev()});
    d.recordSignature=signature(d);await resync();return d.record;
  }
  async function acquire(d){const record=await ensure(d),domain={kind:'device',id:record.device_id},key=deviceKey(session.hostId,domain);
    let lease=store.lease(key);if(!lease){lease=await client.acquire(domain);store.setLease(key,lease);}return {record,domain,key,lease};}
  return {
    async prepare(d,model){if(!confirm(`Prepare ${model.name}: ${model.connect_effects.join(', ')}. On close: ${model.close_effects.join(', ')}.`))return;
      const selected=await acquire(d);await run(selected.domain,selected.record.revision,'connect',{acknowledge_lifecycle:true});},
    async test(d,model,profile){
      if(!confirm(`Test ${model.name} identity in ${host().mode}. Open effects: ${profile.open_effects.join(', ')||'none declared'}. ${profile.probe_mode==='supervised'?'Use the independently prepared supervised session; it will stay owned.':'Close the temporary session after identity read.'}`))return;
      const {record,lease}=await acquire(d);const issued=performance.now();
      const proof=await client.testConnection({draft_id:record.device_id,expected_rev:rev(),consent:{accepted:true,mode:host().mode,config_digest:record.config_digest,
        open_effects:profile.open_effects,supervised:profile.probe_mode==='supervised',retain_session:profile.probe_mode==='supervised'},
        lease_token:lease.token,control_epoch:lease.control_epoch,request_id:crypto.randomUUID().replaceAll('-',''),sequence:client.nextSequence()});
      if(!proof.proof_id)throw new Error('No valid verification proof. Session may remain retained.');
      d.proof={...proof,issued,revision:record.revision,signature:signature(d)};d.message='Connection verified. Add and Save within 60 seconds.';await resync();
    },
    async save(d){if(!canSave(d))throw new Error('Verification expired or fields changed. Test again.');
      const device=await client.saveDevice({draft_id:d.record.device_id,proof_id:d.proof.proof_id,expected_rev:rev()});d.proof=null;await resync();return device;},
    async cancel(d){if(d.record){await client.cancelDraft({draft_id:d.record.device_id,expected_rev:rev()});store.dropLease(deviceKey(session.hostId,{kind:'device',id:d.record.device_id}));await resync();}},
  };
}
