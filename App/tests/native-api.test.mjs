import test from 'node:test';import assert from 'node:assert/strict';
import {createClient} from '../web/api.js';
import * as panels from '../web/panels.js';
test('production API starts only the fixed native Host and has no direct worker path',async()=>{
 const calls=[],client=createClient(async(name,args)=>{calls.push([name,args]);return{};},async()=>()=>{});
 await client.startHost({pythonPath:'untrusted.exe',executable:'other.exe',mode:'simulate'});
 assert.deepEqual(calls,[['host_start',{config:{}}]]);
 assert.equal(client.request,undefined);assert.equal(client.start,undefined);
});
test('production instrument panels do not expose the retired interpreter settings page',()=>{
 assert.equal(panels.settings,undefined);
});
