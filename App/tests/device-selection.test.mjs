import test from 'node:test';import assert from 'node:assert/strict';
import {renderAddWizard} from '../web/setup.js';
import * as actions from '../web/setup-actions.js';
const profile={id:'newport-usb',access:'newport',interfaces:['USB'],fields:{device_key:{kind:'text',required:true}},open_effects:[],probe_mode:'readonly'};
const model={id:'tlb6700',name:'TLB-6700',category:'Laser',profiles:[profile]},catalog={models:[model],categories:['Laser']};
const controllers=[{device_key:'6700 SN1012',serial:'1012'},{device_key:'6700 SN1020',serial:'1020'}];
const draft=()=>({modelId:model.id,profileId:profile.id,params:{},driverCheck:{modelId:model.id,profileId:profile.id,state:'ready',issued:performance.now()},controllerScan:{state:'ready',controllers}});

test('ready laser form shows serial choices without a key input or driver success banner',()=>{
 const html=renderAddWizard(draft(),catalog);
 assert.match(html,/<select[^>]*data-param="device_key"/);assert.match(html,/1012/);assert.match(html,/1020/);
 assert.doesNotMatch(html,/<input[^>]*data-param="device_key"|Controller key format|Required driver|Compatible Newport|Check again/);
});
test('missing driver shows an error and installer action, never a usable Test button',()=>{
 const d=draft();d.driverCheck.state='missing';const html=renderAddWizard(d,catalog);
 assert.match(html,/class="alert"/);assert.match(html,/data-ui="install-draft-driver"/);
 assert.match(html,/<button[^>]*data-ui="test-draft"[^>]*disabled/);
});
test('scan keeps selected serial across reordered results and clears a removed serial and proof',async()=>{
 assert.equal(typeof actions.refreshControllerChoices,'function');
 const d=draft();d.params.device_key=controllers[1].device_key;d.proof={proof_id:'old'};
 await actions.refreshControllerChoices(d,model,profile,async()=>({controllers:[...controllers].reverse()}),()=>{});
 assert.equal(d.params.device_key,controllers[1].device_key);
 await actions.refreshControllerChoices(d,model,profile,async()=>({controllers:[controllers[0]]}),()=>{});
 assert.equal(d.params.device_key,undefined);assert.equal(d.proof,null);
});
test('late scan for a previously selected model cannot populate the new profile',async()=>{
 assert.equal(typeof actions.refreshControllerChoices,'function');
 const d=draft();let finish;const pending=actions.refreshControllerChoices(d,model,profile,()=>new Promise(r=>finish=r),()=>{});
 d.modelId='gain';d.profileId='serial';await actions.refreshControllerChoices(d,{id:'gain'},{access:'serial'},()=>{throw new Error('No scan');},()=>{});
 finish({controllers});await pending;assert.equal(d.controllerScan,null);assert.equal(d.params.device_key,undefined);
});
test('a selected key absent from the current scan cannot test even with a ready SDK',async()=>{
 const calls=[],d=draft();d.params.device_key='6700 SN999';
 const a=actions.createSetupActions({client:{},store:{host:()=>({mode:'real',registry:{registry_rev:0}})}},()=>{calls.push('dialog');return true;},async()=>{});
 await assert.rejects(a.test(d,model,profile),/select.*controller/i);assert.deepEqual(calls,[]);
});
test('late prerequisite completion cannot launch a scan over a newer form selection',async()=>{
 assert.equal(typeof actions.refreshDraftConnection,'function');
 const d=draft();let finish;const calls=[];
 const pending=actions.refreshDraftConnection(d,model,profile,()=>new Promise(r=>finish=r),async()=>{calls.push('old scan');return {controllers};},()=>{});
 d.modelId='gain';d.profileId='serial';await actions.refreshDraftConnection(d,{id:'gain'},{access:'serial'},()=>{throw new Error('No driver read');},()=>{throw new Error('No scan');},()=>{});
 finish({newport:{sdk:{state:'ready'},devices:[{driver_state:'ready'}]}});await pending;
 assert.deepEqual(calls,[]);assert.equal(d.controllerScan,null);
});

test('a single detected controller is selected automatically only when no serial was already selected',async()=>{
 const d=draft();await actions.refreshControllerChoices(d,model,profile,async()=>({controllers:[controllers[0]]}),()=>{});
 assert.equal(d.params.device_key,controllers[0].device_key);
 d.proof={proof_id:'old'};await actions.refreshControllerChoices(d,model,profile,async()=>({controllers:[controllers[1]]}),()=>{});
 assert.equal(d.params.device_key,undefined);assert.equal(d.proof,null);
});

test('an expired scan can be rechecked with Test instead of leaving the form silently disabled',()=>{
 const d=draft();d.params.device_key=controllers[0].device_key;d.driverCheck.issued=-100000;d.controllerScan.issued=-100000;
 const html=renderAddWizard(d,catalog);const button=html.match(/<button[^>]*data-ui="test-draft"[^>]*>/)[0];assert.doesNotMatch(button,/disabled/);
 d.record={revision:1};d.proof={proof_id:'old',issued:-100000,revision:1,signature:JSON.stringify([d.modelId,d.profileId,d.name,d.params])};
 assert.match(renderAddWizard(d,catalog),/Connection check expired\. Test again\./);
});
