import test from 'node:test';import assert from 'node:assert/strict';
let identity={};try{identity=await import('../web/json-value.js');}catch(error){if(error.code!=='ERR_MODULE_NOT_FOUND')throw error;}
test('JSON identity ignores only object property order, including nested domains and contexts',()=>{
 assert.equal(typeof identity.sameJsonValue,'function');
 const left={session_id:'session',domain:{kind:'device',id:'domain'},connection_id:'connection',epoch:1,details:[{b:2,a:1},null]};
 const right={details:[{a:1,b:2},null],epoch:1,connection_id:'connection',domain:{id:'domain',kind:'device'},session_id:'session'};
 assert.equal(identity.sameJsonValue(left,right),true);assert.equal(identity.sameJsonValue(left,structuredClone(left)),true);
});
test('JSON identity retains exact fields, value types and array order',()=>{
 assert.equal(typeof identity.sameJsonValue,'function');const value={domain:{kind:'device',id:'domain'},session_id:'session',connection_id:'connection',epoch:1,steps:[1,2],cleared:null};
 for(const changed of [{...value,epoch:2},{...value,epoch:'1'},{...value,session_id:'other'},{...value,connection_id:null},{...value,domain:{kind:'device',id:'other'}},{...value,domain:{kind:'setup',id:'domain'}},{...value,domain:{kind:'device',id:'domain',extra:true}},{...value,extra:true},{...value,steps:[2,1]},{...value,steps:{0:1,1:2}},{...value,cleared:undefined},Object.fromEntries(Object.entries(value).filter(([key])=>key!=='cleared'))])assert.equal(identity.sameJsonValue(value,changed),false);
 assert.equal(identity.sameJsonValue(null,{}),false);assert.equal(identity.sameJsonValue(false,0),false);assert.equal(identity.sameJsonValue([],{}),false);
 assert.equal(identity.sameJsonValue([],Array(1)),false,'array length remains part of its exact shape');
});
