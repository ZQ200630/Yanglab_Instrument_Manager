import test from 'node:test';
import assert from 'node:assert/strict';
import * as shortcuts from '../web/scan-shortcuts.js';

const defaults = () => ({version:1,enabled:true,bindings:{
 'laser-scan-start':null,'laser-scan-forward':null,'laser-scan-backward':null,'laser-scan-stop':'Escape',
}});
const storage = (initial = null) => {
 let value=initial;
 return {getItem(){return value;},setItem(key,next){value=next;},read(){return value;}};
};
const event = (key,extra={}) => ({key,ctrlKey:false,altKey:false,shiftKey:false,metaKey:false,
 repeat:false,isComposing:false,defaultPrevented:false,target:{tagName:'BUTTON'},
 preventDefault(){this.defaultPrevented=true;},...extra});
const context = (extra={}) => ({isLaser:true,documentFocused:true,modalOpen:false,
 findButton:()=>({disabled:false,click(){}}),isNumericEditor:()=>false,...extra});

test('initial preferences assign only Escape to stop, leaving motion unassigned',()=>{
 assert.deepEqual(shortcuts.defaultScanShortcutPreferences(),defaults());
});

test('bindings normalize key order and case, reject duplicate action keys',()=>{
 const input=defaults();input.bindings['laser-scan-forward']='shift+ctrl+f';
 assert.equal(shortcuts.normalizeScanShortcutPreferences(input).bindings['laser-scan-forward'],'Ctrl+Shift+F');
 input.bindings['laser-scan-backward']='Ctrl+Shift+F';
 assert.throws(()=>shortcuts.normalizeScanShortcutPreferences(input),/already assigned|conflict/i);
});

test('unsupported schema and incomplete or extra fields cannot activate shortcuts',()=>{
 for(const value of [null,{}, {...defaults(),version:2}, {...defaults(),enabled:'true'},
  {...defaults(),bindings:{...defaults().bindings,unknown:'F8'}},
  {...defaults(),extra:true}, {...defaults(),bindings:{'laser-scan-stop':'Escape'}}])
  assert.throws(()=>shortcuts.normalizeScanShortcutPreferences(value),/preferences|version|field|binding|enabled/i);
});

test('key capture requires an intentional supported combo and consumes the event',()=>{
 const combo=event('f',{ctrlKey:true,shiftKey:true});
 assert.equal(shortcuts.captureScanShortcut(combo),'Ctrl+Shift+F');assert.equal(combo.defaultPrevented,true);
 assert.equal(shortcuts.captureScanShortcut(event('Escape')),'Escape');
 assert.equal(shortcuts.captureScanShortcut(event('F8')),'F8');
});

test('key capture rejects browser destructive keys, typing, modifier-only and AltGraph',()=>{
 for(const ev of [event('f'),event('Shift',{shiftKey:true}),event('F5'),event('F11'),event('F12'),
  event('w',{ctrlKey:true}),event('r',{ctrlKey:true}),event('t',{ctrlKey:true}),
  event('F4',{altKey:true}),event('ArrowLeft',{altKey:true}),event('f',{metaKey:true}),
  event('f',{ctrlKey:true,altKey:true}),event('Dead'),event('Process'),event('f',{isComposing:true})]){
  assert.throws(()=>shortcuts.captureScanShortcut(ev),/supported|modifier|browser|composition|key/i);
  assert.equal(ev.defaultPrevented,true,'capture fields consume even rejected keys');
 }
});

test('saved preferences roundtrip; invalid stored settings disable shortcuts visibly',()=>{
 const memory=storage(),controller=shortcuts.createScanShortcuts({storage:memory});
 controller.bind('laser-scan-forward','Ctrl+Shift+F');controller.update({...controller.preferences,enabled:false});
 const restored=shortcuts.loadScanShortcutPreferences(memory);
 assert.equal(restored.preferences.enabled,false);assert.equal(restored.preferences.bindings['laser-scan-forward'],'Ctrl+Shift+F');
 const invalid=shortcuts.loadScanShortcutPreferences(storage('{invalid'));
 assert.equal(invalid.preferences.enabled,false);assert.match(invalid.error,/stored|saved|read/i);
 const unknown=shortcuts.loadScanShortcutPreferences(storage(JSON.stringify({...defaults(),version:99})));
 assert.equal(unknown.preferences.enabled,false);assert.match(unknown.error,/version|stored|saved/i);
});

test('failed writes preserve the active preferences and report failure without pretending to save',()=>{
 const errors=[],changes=[],controller=shortcuts.createScanShortcuts({storage:{getItem:()=>null,setItem(){throw Error('quota denied');}},
  onError:error=>errors.push(error),onChange:p=>changes.push(p)});
 assert.equal(controller.bind('laser-scan-forward','F8'),false);
 assert.deepEqual(controller.preferences,defaults());assert.equal(changes.length,0);
 assert.match(errors[0],/save|quota denied/i);
});

test('unassign and reset save explicit changes; snapshots cannot mutate active bindings',()=>{
 const memory=storage(),controller=shortcuts.createScanShortcuts({storage:memory});
 controller.bind('laser-scan-forward','F8');controller.unassign('laser-scan-stop');
 assert.equal(controller.preferences.bindings['laser-scan-stop'],null);
 const snapshot=controller.preferences;snapshot.bindings['laser-scan-forward']='F9';
 assert.equal(controller.preferences.bindings['laser-scan-forward'],'F8');
 assert.equal(controller.reset(),true);assert.deepEqual(controller.preferences,defaults());
 assert.deepEqual(JSON.parse(memory.read()),defaults());
});

test('validation failure retains previous configuration and reports the conflict',()=>{
 const errors=[],controller=shortcuts.createScanShortcuts({storage:storage(),onError:error=>errors.push(error)});
 assert.equal(controller.bind('laser-scan-forward','Escape'),false);
 assert.deepEqual(controller.preferences,defaults());assert.match(errors[0],/assigned|conflict/i);
 assert.equal(controller.bind('unknown','F8'),false);assert.match(errors[1],/action|binding/i);
});

test('dispatch routes only to a matching enabled button once and consumes handled keys',()=>{
 const seen=[],controller=shortcuts.createScanShortcuts({storage:storage()});
 controller.bind('laser-scan-forward','Ctrl+Shift+F');
 const ev=event('F',{ctrlKey:true,shiftKey:true});
 assert.equal(controller.handleKeydown(ev,context({findButton:op=>({disabled:false,click(){seen.push(op);}})})),true);
 assert.deepEqual(seen,['laser-scan-forward']);assert.equal(ev.defaultPrevented,true);
 assert.equal(controller.handleKeydown(event('F',{ctrlKey:true,shiftKey:true}),context({findButton:()=>null})),false);
 assert.equal(controller.handleKeydown(event('F',{ctrlKey:true,shiftKey:true}),context({findButton:()=>({disabled:true,click(){seen.push('bad');}})})),false);
 assert.deepEqual(seen,['laser-scan-forward']);
});

test('dispatch ignores disabled preferences, background/non-laser context, modals and repeated/composing keys',()=>{
 const seen=[],controller=shortcuts.createScanShortcuts({storage:storage()});
 const safe=context({findButton:()=>({disabled:false,click(){seen.push('bad');}})});
 for(const extra of [{isLaser:false},{documentFocused:false},{modalOpen:true}])
  assert.equal(controller.handleKeydown(event('Escape'),{...safe,...extra}),false);
 for(const extra of [{repeat:true},{isComposing:true},{defaultPrevented:true}])
  assert.equal(controller.handleKeydown(event('Escape',extra),safe),false);
 controller.update({...controller.preferences,enabled:false});
 assert.equal(controller.handleKeydown(event('Escape'),safe),false);assert.deepEqual(seen,[]);
});

test('editing suppresses scan motion; Escape stop works only in numeric scan editors',()=>{
 const seen=[],controller=shortcuts.createScanShortcuts({storage:storage()});controller.bind('laser-scan-forward','F8');
 const safe=context({findButton:op=>({disabled:false,click(){seen.push(op);}}),isNumericEditor:el=>el.id==='laser-scan-start'});
 for(const target of [{tagName:'INPUT',id:'unrelated'},{tagName:'SELECT'},{tagName:'TEXTAREA'},
  {tagName:'SPAN',isContentEditable:true},{tagName:'INPUT',id:'laser-scan-start'}])
  assert.equal(controller.handleKeydown(event('F8',{target}),safe),false);
 assert.equal(controller.handleKeydown(event('Escape',{target:{tagName:'INPUT',id:'unrelated'}}),safe),false);
 assert.equal(controller.handleKeydown(event('Escape',{target:{tagName:'INPUT',id:'laser-scan-start'}}),safe),true);
 assert.deepEqual(seen,['laser-scan-stop']);
});

test('capture and dialog descendants always suppress shortcuts, including Escape',()=>{
 const controller=shortcuts.createScanShortcuts({storage:storage()});
 for(const target of [
  {tagName:'INPUT',id:'laser-scan-start',dataset:{scanShortcutCapture:'laser-scan-stop'}},
  {tagName:'SPAN',closest:selector=>selector.includes('dialog')?{}:null},
  {tagName:'DIV',isContentEditable:true,id:'laser-scan-start'},
 ])assert.equal(controller.handleKeydown(event('Escape',{target}),context({isNumericEditor:()=>true})),false);
});

test('hints show actual bindings, disabled status and no raw unescaped preference content',()=>{
 assert.equal(shortcuts.scanShortcutHint(defaults(),'laser-scan-stop'),'Esc');
 assert.equal(shortcuts.scanShortcutHint(defaults(),'laser-scan-forward'),'Unassigned');
 assert.equal(shortcuts.scanShortcutHint({...defaults(),enabled:false},'laser-scan-stop'),'Shortcuts off');
 const html=shortcuts.renderScanShortcuts(defaults());
 assert.match(html,/<details/);assert.match(html,/data-scan-shortcuts-enabled[^>]*checked/);
 assert.match(html,/data-scan-shortcut-capture="laser-scan-stop"[^>]*readonly/);
 for(const action of ['laser-scan-start','laser-scan-forward','laser-scan-backward','laser-scan-stop']){
  assert.match(html,new RegExp('data-scan-shortcut-capture="'+action+'"'));
  assert.match(html,new RegExp('data-scan-shortcut-unassign="'+action+'"'));
 }
 assert.match(html,/data-scan-shortcuts-reset/);
});

test('saved shortcut controls opt out of stale draft restoration; the folding panel has a stable identity',()=>{
 const html=shortcuts.renderScanShortcuts(defaults());
 assert.match(html,/<details[^>]*id="laser-scan-shortcuts"/);
 for(const tag of [...html.matchAll(/<input\b[^>]*>/g)].map(match=>match[0]))
  assert.match(tag,/data-managed="true"/,'saved shortcuts must not be overwritten by pre-save field snapshots');
});
