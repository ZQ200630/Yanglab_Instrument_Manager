import test from 'node:test';
import assert from 'node:assert/strict';
import * as activity from '../web/console-ui.js';

test('unknown-duration instrument work has elapsed feedback, never a made-up percentage',()=>{
 assert.equal(typeof activity.startActivity,'function');
 const work=activity.startActivity('read_trace','instrument',100);
 const html=activity.renderActivity(work,6100);
 assert.match(html,/Reading &amp; saving/);assert.match(html,/6 s/);
 assert.match(html,/Still working/);assert.match(html,/role="status"/);
 assert.doesNotMatch(html,/value="|\d+%/);
});
test('only received archive bytes produce determinate download progress',()=>{
 assert.equal(typeof activity.startActivity,'function');
 const work=activity.advanceActivity(activity.startActivity('load','manifest',0),'download',10,{received:16384,total:32768});
 const html=activity.renderActivity(work,20);
 assert.match(html,/<progress[^>]*value="16384"[^>]*max="32768"/);assert.match(html,/50%/);
 assert.doesNotMatch(activity.renderActivity(activity.advanceActivity(work,'verify',30),40),/value="16384"|50%/);
});
test('phase durations remain separate and unknown outcome never reads as completion',()=>{
 assert.equal(typeof activity.startActivity,'function');
 let work=activity.startActivity('read_trace','prepare',0);
 work=activity.advanceActivity(work,'instrument',100);
 work=activity.advanceActivity(work,'sync',1100);
 work=activity.finishActivity(work,'unknown',1200);
 assert.deepEqual(work.timings,{prepare:100,instrument:1000,sync:100});
 const html=activity.renderActivity(work,9000);
 assert.match(html,/Outcome unknown/);assert.doesNotMatch(html,/spinner|Capture ready|Still working/);
});
test('elapsed ticks update just feedback nodes, not the surrounding editable form',()=>{
 assert.equal(typeof activity.tickActivities,'function');
 const elapsed={dataset:{activityElapsed:'100',activityEnded:''},textContent:''};
 const slow={dataset:{activitySlow:'100'},hidden:true};
 const root={querySelectorAll:selector=>selector==='[data-activity-elapsed]'?[elapsed]:[slow],set innerHTML(_){assert.fail('Timer must not replace a page');}};
 activity.tickActivities(root,7100);assert.equal(elapsed.textContent,'7 s');assert.equal(slow.hidden,false);
});
