import test from 'node:test';import assert from 'node:assert/strict';
import {canSave,renderAddWizard,renderDeviceSetup,renderSettings,instrumentTarget,renderCheckPolicy,renderDriverStatus} from '../web/setup.js';
const model={id:'gain',manufacturer:'Yang Lab',name:'Gain Driver',category:'Custom',profiles:[{id:'cp210x-serial',interfaces:['Serial'],access:'serial',probe_mode:'supervised',open_effects:['DTR_RTS_reset_not_verified'],fields:{port:{kind:'serial',required:true}}}]};
test('wizard_requires_current_proof_before_save',()=>{
  const draft={modelId:'gain',profileId:'cp210x-serial',name:'Gain',params:{port:'COM7'},record:{revision:1},proof:{proof_id:'proof',issued:1000,revision:1,signature:JSON.stringify(['gain','cp210x-serial','Gain',{port:'COM7'}])}};
  assert.equal(canSave(draft,2000),true);assert.equal(canSave(draft,61000),false);
  assert.equal(canSave({...draft,params:{port:'COM8'}},2000),false);
});
test('refresh controls expose interval and immediate refresh without internal check switches',()=>{
  const html=renderCheckPolicy({device_id:'d',model_id:'gain',profile_id:'cp210x-serial',check_policy:{interval_s:30}},{models:[model]});
  assert.match(html,/Refresh interval/);assert.match(html,/Refresh now/);
  assert.doesNotMatch(html,/Enumerate|identity probe|check-readonly|check-enumeration|check policy/);
});
test('unsupported_model_stays_draft_and_unsafe_probe_is_disclosed',()=>{
  assert.match(renderAddWizard({modelId:'unknown'},{categories:['ESA'],models:[]}),/Driver required/);
  assert.match(renderAddWizard({modelId:'gain',profileId:'cp210x-serial',params:{}},{categories:['Custom'],models:[model]}),/Prepare session/);
});
test('fiber_controller_routes_to_owning_setup_without_raw_motion',()=>{
  const record={device_id:'1'.repeat(32),name:'MDT',model_id:'mdt693b',params:{port:'COM1'}},setup={setup_id:'2'.repeat(32),members:[record.device_id]};
  assert.deepEqual(instrumentTarget(record,[setup]),{kind:'setup',id:setup.setup_id});
  assert.equal(renderDeviceSetup({registry:{devices:[],drafts:[],setups:[]}}).includes('OSA AQ6370'),false);
  assert.match(renderDeviceSetup({host_id:'a'.repeat(32),registry:{devices:[record],drafts:[],setups:[setup]}}),/Add New/);
});
test('normal draft hides release machinery and failed cleanup keeps an exception recovery action',()=>{
  const draft={modelId:'gain',profileId:'cp210x-serial',params:{},record:{device_id:'1'.repeat(32),revision:1}},catalog={categories:['Custom'],models:[model]};
  assert.doesNotMatch(renderAddWizard(draft,catalog),/safe-stop-draft|release draft|within 60 seconds/);
  assert.match(renderAddWizard({...draft,releaseError:'Release failed'},catalog),/safe-stop-draft/);
});
test('driver maintenance belongs in Settings rather than the configured device list',()=>{
  const host={connected:true,registry:{devices:[],drafts:[],setups:[]}};
  assert.doesNotMatch(renderDeviceSetup(host),/data-ui="check-drivers"/);
  assert.match(renderSettings(host,{}),/data-ui="check-drivers"/);
});
const laser={id:'tlb6700',name:'TLB-6700',manufacturer:'Newport',category:'Laser',profiles:[{id:'newport-usb',interfaces:['USB'],access:'newport',probe_mode:'readonly',open_effects:[],fields:{}}]};
test('Newport connection testing waits for the selected profile prerequisite check',()=>{
  const d={modelId:'tlb6700',profileId:'newport-usb',params:{}};
  const catalog={categories:['Laser'],models:[laser]};
  assert.match(renderAddWizard(d,catalog),/data-ui="test-draft"[^>]*disabled/);
  assert.match(renderAddWizard({...d,params:{device_key:'6700 SN1'},controllerScan:{state:'ready',issued:performance.now(),controllers:[{device_key:'6700 SN1',serial:'1'}]},driverCheck:{modelId:'tlb6700',profileId:'newport-usb',state:'ready',issued:performance.now()}},catalog),/data-ui="test-draft"\s*>/);
  assert.match(renderAddWizard({...d,driverCheck:{modelId:'tlb6700',profileId:'other',state:'ready',issued:performance.now()}},catalog),/data-ui="test-draft"[^>]*disabled/);
});
test('device card uses custom name then model and serial without identity-strength jargon',()=>{
 const d={device_id:'a'.repeat(32),name:'1060 nm bench laser',model_id:'tlb6700',profile_id:'newport-usb',params:{device_key:'6700 SN22500001'},expected_identity:{serial:'22500001'},identity_strength:'strong'};
 const html=renderDeviceSetup({connected:true,host_id:'b'.repeat(32),registry:{devices:[d],drafts:[],setups:[]}},{models:[laser]});
 assert.match(html,/<h2>1060 nm bench laser<\/h2>/);assert.match(html,/TLB-6700 · S\/N 22500001/);
 assert.doesNotMatch(html,/· strong|6700 SN22500001|Fiber setups/);
});

test('driver settings show concise installation state and only actionable installation controls',()=>{
 const ready=renderDriverStatus({newport:{sdk:{state:'ready',message:'internal SDK path'},devices:[{description:'Tunable laser',driver_state:'ready',problem_code:0}]}});
 assert.match(ready,/Newport USB/);assert.match(ready,/Installed/);
 assert.doesNotMatch(ready,/internal SDK path|problem code|NI-VISA|data-ui="install-driver"/);
 const missing=renderDriverStatus({newport:{sdk:{state:'missing'},devices:[]}});
 assert.match(missing,/Not installed/);assert.match(missing,/data-ui="install-driver"/);
 assert.doesNotMatch(renderAddWizard({}, {categories:['OSA'],models:[]}),/Driver required/);
});
