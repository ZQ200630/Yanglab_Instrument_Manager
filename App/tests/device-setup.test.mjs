import test from 'node:test';import assert from 'node:assert/strict';
import {canSave,renderAddWizard,renderDeviceSetup,instrumentTarget,renderCheckPolicy} from '../web/setup.js';
const model={id:'gain',manufacturer:'Yang Lab',name:'Gain Driver',category:'Custom',profiles:[{id:'cp210x-serial',interfaces:['Serial'],access:'serial',probe_mode:'supervised',open_effects:['DTR_RTS_reset_not_verified'],fields:{port:{kind:'serial',required:true}}}]};
test('wizard_requires_current_proof_before_save',()=>{
  const draft={modelId:'gain',profileId:'cp210x-serial',name:'Gain',params:{port:'COM7'},record:{revision:1},proof:{proof_id:'proof',issued:1000,revision:1,signature:JSON.stringify(['gain','cp210x-serial','Gain',{port:'COM7'}])}};
  assert.equal(canSave(draft,2000),true);assert.equal(canSave(draft,61000),false);
  assert.equal(canSave({...draft,params:{port:'COM8'}},2000),false);
});
test('online checks show separate stage consent and unsafe profiles cannot auto-open',()=>{
  const html=renderCheckPolicy({device_id:'d',model_id:'gain',profile_id:'cp210x-serial',check_policy:{interval_s:30}},{models:[model]});
  assert.match(html,/Enumerate/);assert.match(html,/Read-only/);assert.match(html,/id="check-readonly-d"[^>]*disabled/);
});
test('unsupported_model_stays_draft_and_unsafe_probe_is_disclosed',()=>{
  assert.match(renderAddWizard({modelId:'unknown'},{categories:['ESA'],models:[]}),/Driver required/);
  assert.match(renderAddWizard({modelId:'gain',profileId:'cp210x-serial',params:{}},{categories:['Custom'],models:[model]}),/Manual verification required/);
});
test('fiber_controller_routes_to_owning_setup_without_raw_motion',()=>{
  const record={device_id:'1'.repeat(32),name:'MDT',model_id:'mdt693b',params:{port:'COM1'}},setup={setup_id:'2'.repeat(32),members:[record.device_id]};
  assert.deepEqual(instrumentTarget(record,[setup]),{kind:'setup',id:setup.setup_id});
  assert.equal(renderDeviceSetup({registry:{devices:[],drafts:[],setups:[]}}).includes('OSA AQ6370'),false);
  assert.match(renderDeviceSetup({host_id:'a'.repeat(32),registry:{devices:[record],drafts:[],setups:[setup]}}),/Add New/);
});
test('a retained draft always keeps a separate safe-release action',()=>{
  assert.match(renderAddWizard({modelId:'gain',profileId:'cp210x-serial',params:{},record:{device_id:'1'.repeat(32),revision:1}},{categories:['Custom'],models:[model]}),/safe-stop-draft/);
});
