import test from 'node:test';
import assert from 'node:assert/strict';
import * as renderer from '../web/console-ui.js';

function clock(render){const pending=new Map();let seq=0;const r=renderer.createRenderScheduler(render,callback=>{pending.set(++seq,callback);return seq},id=>pending.delete(id));return {r,pending,frame(){const callbacks=[...pending.values()];pending.clear();for(const callback of callbacks)callback()}};}
test('a burst of background changes paints once with the newest state',()=>{
 assert.equal(typeof renderer.createRenderScheduler,'function','background scheduling is missing');
 let state=0,seen=[];const f=clock(()=>seen.push(state));for(let i=0;i<100;i++){state=i;f.r.request()}
 assert.deepEqual(seen,[]);assert.equal(f.pending.size,1);f.frame();assert.deepEqual(seen,[99]);f.frame();assert.deepEqual(seen,[99]);
});
test('a click flushes progress immediately without an old scheduled repaint',()=>{
 assert.equal(typeof renderer.createRenderScheduler,'function');
 let busy=false,seen=[];const f=clock(()=>seen.push(busy));f.r.request();busy=true;f.r.flush();assert.deepEqual(seen,[true]);assert.equal(f.pending.size,0);f.frame();assert.deepEqual(seen,[true]);
});
test('closing cancels background rendering and cannot retain a live callback',()=>{
 assert.equal(typeof renderer.createRenderScheduler,'function');
 let renders=0;const f=clock(()=>renders++);f.r.request();f.r.cancel();f.frame();assert.equal(renders,0);
});
