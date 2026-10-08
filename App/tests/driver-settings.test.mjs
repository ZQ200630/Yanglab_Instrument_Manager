import test from 'node:test';
import assert from 'node:assert/strict';
import {renderDriverStatus} from '../web/setup.js';

const inventory={newport:{sdk:{state:'ready'},devices:[]},usb_serial:{ch340:{state:'ready'},cp210x:{state:'missing'}}};
test('driver settings expose an explicit install action beside the missing package',()=>{
 const html=renderDriverStatus(inventory);
 assert.match(html,/data-ui="install-driver" data-driver="cp210x"/);
 assert.doesNotMatch(html,/data-ui="install-driver" data-driver="(?:ch340|newport)"/);
 assert.match(html,/Driver required/);assert.doesNotMatch(html,/Not installed|class="device-row"/);
 assert.match(html,/Gain Driver/);assert.match(html,/driver license/);
});
test('absent adapters and failed inventory never masquerade as missing drivers',()=>{
 for(const state of ['not_detected','unavailable']){
  const html=renderDriverStatus({...inventory,usb_serial:{ch340:{state:'ready'},cp210x:{state}}});
  assert.doesNotMatch(html,/data-ui="install-driver"/);
  assert.match(html,state==='not_detected'?/No device detected/:/Check unavailable/);
 }
 const failed=renderDriverStatus({...inventory,errors:{usb_serial:'metadata unavailable'}});
 assert.doesNotMatch(failed,/data-ui="install-driver"/);
});
test('refresh, offline and active installation block additional installer launches',()=>{
 for(const [value,available,installation] of [[{...inventory,checking:true},true,{}],[inventory,false,{}],[inventory,true,{driver:'cp210x',installing:true}]]){
  const html=renderDriverStatus(value,available,installation);
  for(const button of html.matchAll(/<button([^>]*data-ui="install-driver"[^>]*)>/g))assert.match(button[1],/\bdisabled\b/);
  if(installation.installing){assert.match(html,/Installing…/);assert.match(html,/aria-busy="true"/);}
 }
});
