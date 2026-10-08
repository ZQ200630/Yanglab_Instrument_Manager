// Explicit offline bridge check: consumes statuses produced by real Rust
// scheduler + native driver adapters with finite instrument byte transports.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {instanceView} from '../web/instance-view.js';
import {canResume,canSendNormal} from '../web/control-state.js';
import {connectionAction} from '../web/panels.js';
const cases=JSON.parse(readFileSync(process.argv[2],'utf8'));
assert.equal(cases.length,6);
for(const c of cases){
 const domain={...c.held.domains[c.key],device:c.held.devices[c.key]};
 const held=instanceView(c.kind,domain,true,0,{});held.hideConnectionAction=true;
 assert.equal(canResume(held.roles[c.kind]),true);
 assert.match(connectionAction(c.kind,held),/Resume controls/);
 const resumed=instanceView(c.kind,c.resumed.domains[c.key],true,0,{});
 assert.equal(canSendNormal(resumed.roles[c.kind]),true);
 assert.deepEqual(c.resumed.domains[c.key].safety,domain.safety);
}
process.stdout.write('6 native status → UI → resume cases passed; original safety evidence retained.\n');
