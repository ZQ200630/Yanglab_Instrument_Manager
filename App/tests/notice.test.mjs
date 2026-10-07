import test from 'node:test';import assert from 'node:assert/strict';import * as notices from '../web/notice.js';
test('notification expires, left-click dismisses, and right-click copies its exact text',async()=>{
 assert.equal(typeof notices.createNotice,'function');
 const events={},node={hidden:true,addEventListener:(n,f)=>events[n]=f},tasks=new Map(),copied=[];let seq=0;
 const notice=notices.createNotice(node,{schedule:(f,ms)=>{assert.equal(ms,5000);tasks.set(++seq,f);return seq;},cancel:id=>tasks.delete(id),copy:async text=>copied.push(text)});
 notice.show('Connection failed: USB busy',true);assert.equal(node.className,'notice error');assert.equal(node.hidden,false);
 let prevented=false;await events.contextmenu({preventDefault:()=>prevented=true});assert.equal(prevented,true);assert.deepEqual(copied,['Connection failed: USB busy']);
 events.click();assert.equal(node.hidden,true);assert.equal(tasks.size,0);
 notice.show('First');notice.show('Second');assert.equal(tasks.size,1);[...tasks.values()][0]();assert.equal(node.hidden,true);
});
