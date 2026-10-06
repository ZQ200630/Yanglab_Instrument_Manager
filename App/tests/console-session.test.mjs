import test from 'node:test';import assert from 'node:assert/strict';import {createConsoleSession} from '../web/main.js';
test('observer heartbeat never rebuilds an unchanged disconnected form',async()=>{let renders=0;const session=createConsoleSession({},()=>renders++);await session.heartbeat();assert.equal(renders,0);});
function controlledSession(renew){
 const h='a'.repeat(32),b='b'.repeat(32),d='c'.repeat(32),client='d'.repeat(32),domain={kind:'device',id:d},key=h+'/device/'+d;
 const session=createConsoleSession({renew});const context={session_id:'f'.repeat(32),domain,connection_id:'e'.repeat(32),epoch:1};
 const control={['device:'+d]:{state:'CONTROLLED',controller_session:client,control_epoch:0}};
 session.apply({type:'snapshot',host_id:h,boot_id:b,seq:1,data:{domains:{['device:'+d]:{context,state:'READY'}},control}});
 const lease={token:'1'.repeat(32),boot_id:b,session_id:client,domain,control_epoch:0,expires_in_ms:10000};session.store.setLease(key,lease);
 return {session,h,b,d,key,context,lease};
}
test('late heartbeat cannot restore a lease removed by disconnect',async()=>{
 let resolve;const pending=new Promise(yes=>resolve=yes);const f=controlledSession(()=>pending);
 const work=f.session.heartbeat();f.session.offline();resolve(f.lease);await work;
 assert.equal(f.session.store.lease(f.key),null);
});
test('authoritative owner/epoch changes invalidate control before renewal replies',async()=>{
 let resolve;const pending=new Promise(yes=>resolve=yes);const f=controlledSession(()=>pending);
 assert.equal(f.session.store.canControl(f.key),true);const work=f.session.heartbeat();
 f.session.apply({type:'snapshot',host_id:f.h,boot_id:f.b,seq:2,data:{domains:{['device:'+f.d]:{context:f.context,state:'READY'}},control:{['device:'+f.d]:{state:'AVAILABLE',controller_session:null,control_epoch:1}}}});
 assert.equal(f.session.store.canControl(f.key),false);resolve(f.lease);await work;
 assert.equal(f.session.store.lease(f.key),null);assert.equal(f.session.store.canControl(f.key),false);
});
