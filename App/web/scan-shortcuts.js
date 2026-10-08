// Keyboard preferences never send instrument commands; dispatch follows an enabled UI button.
export const SCAN_SHORTCUT_STORAGE_KEY='yanglab.scan-shortcuts.v1';
export const SCAN_SHORTCUT_ACTIONS=Object.freeze({
 'laser-scan-start':'Full scan',
 'laser-scan-forward':'Current → Stop',
 'laser-scan-backward':'Current → Start',
 'laser-scan-stop':'Stop scan',
});
const actions=Object.keys(SCAN_SHORTCUT_ACTIONS);
const modifiers=['Ctrl','Alt','Shift'];
const numericEditors=new Set(['laser-wavelength','laser-scan-start','laser-scan-stop','laser-scan-speed','laser-scan-return-speed']);
const reserved=new Set([
 'Ctrl+W','Ctrl+Q','Ctrl+N','Ctrl+T','Ctrl+R','Ctrl+L','Ctrl+E','Ctrl+P','Ctrl+S','Ctrl+F',
 'Ctrl+H','Ctrl+J','Ctrl+D','Ctrl+U','Ctrl+Shift+W','Ctrl+Shift+Q','Ctrl+Shift+N',
 'Ctrl+Shift+T','Ctrl+Shift+R','Ctrl+Shift+J','Ctrl+Shift+I','Ctrl+Shift+C',
 'Alt+F4',
]);
const esc=value=>String(value).replace(/[&<>"']/g,char=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[char]));

export function defaultScanShortcutPreferences(){
 return {version:1,enabled:true,bindings:Object.fromEntries(actions.map(action=>[action,action==='laser-scan-stop'?'Escape':null]))};
}

function normalizeBinding(binding){
 if(typeof binding!=='string'||binding.length>48)throw Error('Shortcut binding must be a supported key combination.');
 const parts=binding.split('+').map(part=>part.trim());
 const raw=parts.pop(),lower=raw?.toLowerCase();
 const key=lower==='escape'||lower==='esc'?'Escape':/^f(?:[1-9]|1[0-2])$/i.test(raw||'')?raw.toUpperCase():/^[a-z0-9]$/i.test(raw||'')?raw.toUpperCase():null;
 if(!key)throw Error('Use Escape, a supported F key, or Ctrl/Alt with a letter or number. Modifier-only keys are unsupported.');
 const selected=[];
 for(const part of parts){
  const modifier=modifiers.find(value=>value.toLowerCase()===part.toLowerCase());
  if(!modifier||selected.includes(modifier))throw Error('Unsupported or duplicate shortcut modifier.');
  selected.push(modifier);
 }
 if(selected.includes('Ctrl')&&selected.includes('Alt'))throw Error('Ctrl+Alt and AltGraph key combinations are unsupported.');
 if(key==='Escape'&&selected.length)throw Error('Use Escape without modifiers.');
 if(/^[A-Z0-9]$/.test(key)&&!selected.some(value=>value==='Ctrl'||value==='Alt'))throw Error('Letter and number shortcuts require Ctrl or Alt; ordinary typing keys are unsupported.');
 const normalized=[...modifiers.filter(value=>selected.includes(value)),key].join('+');
 if(['F5','F11','F12'].includes(key)||reserved.has(normalized))throw Error('This key combination is reserved by the browser.');
 return normalized;
}

export function normalizeScanShortcutPreferences(value){
 if(!value||typeof value!=='object'||Array.isArray(value))throw Error('Invalid shortcut preferences.');
 if(value.version!==1)throw Error('Unsupported shortcut preferences version.');
 if(Object.keys(value).sort().join(',')!=='bindings,enabled,version')throw Error('Invalid shortcut preferences fields.');
 if(typeof value.enabled!=='boolean')throw Error('Shortcut enabled must be true or false.');
 if(!value.bindings||typeof value.bindings!=='object'||Array.isArray(value.bindings)||
    Object.keys(value.bindings).sort().join(',')!==[...actions].sort().join(','))throw Error('Invalid shortcut binding fields.');
 const bindings={},used=new Map();
 for(const action of actions){
  const binding=value.bindings[action]===null?null:normalizeBinding(value.bindings[action]);
  if(binding&&used.has(binding))throw Error(`${binding} is already assigned to ${SCAN_SHORTCUT_ACTIONS[used.get(binding)]}.`);
  bindings[action]=binding;if(binding)used.set(binding,action);
 }
 return {version:1,enabled:value.enabled,bindings};
}

function browserStorage(){return globalThis.localStorage;}

export function loadScanShortcutPreferences(storage){
 try{
  const raw=(storage??browserStorage()).getItem(SCAN_SHORTCUT_STORAGE_KEY);
  return {preferences:raw===null?defaultScanShortcutPreferences():normalizeScanShortcutPreferences(JSON.parse(raw)),error:null};
 }catch(cause){
  return {preferences:{...defaultScanShortcutPreferences(),enabled:false},error:`Could not read saved scan shortcuts: ${cause.message||cause}`};
 }
}

export function saveScanShortcutPreferences(storage,preferences){
 const normalized=normalizeScanShortcutPreferences(preferences);
 try{(storage??browserStorage()).setItem(SCAN_SHORTCUT_STORAGE_KEY,JSON.stringify(normalized));}
 catch(cause){throw Error(`Could not save scan shortcuts: ${cause.message||cause}`);}
 return normalized;
}

function eventBinding(event){
 if(event.isComposing||event.key==='Process'||event.key==='Dead')throw Error('Composition keys are unsupported.');
 if(event.metaKey||event.getModifierState?.('AltGraph'))throw Error('Meta and AltGraph modifiers are unsupported.');
 return normalizeBinding([...(event.ctrlKey?['Ctrl']:[]),...(event.altKey?['Alt']:[]),...(event.shiftKey?['Shift']:[]),event.key].join('+'));
}

// Capture handlers must call this before the application's shortcut dispatch listener.
export function captureScanShortcut(event){
 event.preventDefault();
 if(event.repeat)throw Error('Repeated keys are unsupported. Press the shortcut once.');
 return eventBinding(event);
}

function editableTarget(target){
 return ['INPUT','SELECT','TEXTAREA'].includes(target?.tagName)||target?.isContentEditable===true||
  !!target?.closest?.('[contenteditable]:not([contenteditable="false"])');
}

function canDispatch(event,context,action,binding){
 if(context?.isLaser!==true||context.documentFocused!==true||context.modalOpen!==false||
  event.defaultPrevented||event.repeat||event.isComposing)return false;
 const target=event.target;
 if(target?.dataset?.scanShortcutCapture!==undefined||target?.closest?.('[data-scan-shortcut-capture]')||
    target?.closest?.('dialog,[role="dialog"],[aria-modal="true"]'))return false;
 if(!editableTarget(target))return true;
 const numeric=target?.tagName==='INPUT'&&(context.isNumericEditor?context.isNumericEditor(target):numericEditors.has(target.id));
 return action==='laser-scan-stop'&&binding==='Escape'&&numeric&&target?.isContentEditable!==true;
}

export function createScanShortcuts({storage,onChange=()=>{},onError=()=>{}}={}){
 const loaded=loadScanShortcutPreferences(storage);
 let preferences=loaded.preferences,lastError=loaded.error;
 const snapshot=()=>({version:1,enabled:preferences.enabled,bindings:{...preferences.bindings}});
 function report(cause){lastError=cause.message||String(cause);onError(lastError);return false;}
 function update(next){
  let saved;
  try{saved=saveScanShortcutPreferences(storage,next);}catch(cause){return report(cause);}
  preferences=saved;lastError=null;onChange(snapshot());return true;
 }
 function bind(action,binding){
  if(!Object.hasOwn(SCAN_SHORTCUT_ACTIONS,action))return report(Error('Unknown scan shortcut action.'));
  return update({...snapshot(),bindings:{...preferences.bindings,[action]:binding}});
 }
 if(lastError)onError(lastError);
 return {
  get preferences(){return snapshot();},get error(){return lastError;},
  update,bind,unassign:action=>bind(action,null),reset:()=>update(defaultScanShortcutPreferences()),
  handleKeydown(event,context){
   if(!preferences.enabled||event.defaultPrevented||event.repeat||event.isComposing)return false;
   let binding;try{binding=eventBinding(event);}catch{return false;}
   const action=actions.find(value=>preferences.bindings[value]===binding);
   if(!action||!canDispatch(event,context,action,binding))return false;
   const button=context.findButton?.(action);
   if(!button||button.disabled||button.hidden||button.getAttribute?.('aria-disabled')==='true'||
      button.matches?.(':disabled')||button.closest?.('[inert]')||
      (button.dataset?.op!==undefined&&button.dataset.op!==action)||typeof button.click!=='function')return false;
   event.preventDefault();button.click();return true;
  },
 };
}

export function scanShortcutHint(preferences,action){
 if(!preferences.enabled)return 'Shortcuts off';
 return (preferences.bindings[action]||'Unassigned').replace(/^Escape$/,'Esc');
}

export function renderScanShortcutHint(preferences,action){
 const hint=scanShortcutHint(preferences,action);
 return `<small class="scan-shortcut-hint" data-scan-shortcut-hint="${esc(action)}" aria-label="Shortcut: ${esc(hint)}">${esc(hint)}</small>`;
}

export function renderScanShortcuts(preferences,{error='',open=false}={}){
 const normalized=normalizeScanShortcutPreferences(preferences);
 return `<details id="laser-scan-shortcuts" class="scan-shortcut-settings" data-scan-shortcuts-settings${open?' open':''}>
 <summary>Scan shortcuts · ${normalized.enabled?'On':'Off'}</summary>
 <label class="scan-shortcut-toggle"><input type="checkbox" data-scan-shortcuts-enabled data-managed="true"${normalized.enabled?' checked':''}> Enable scan shortcuts</label>
 <p class="scan-shortcut-help">Focus a binding and press Esc, an F key, or Ctrl/Alt with a letter or number. Reserved browser keys are unavailable. Scan motion shortcuts do not run while editing fields.</p>
 <div class="scan-shortcut-bindings">${actions.map(action=>`<div class="scan-shortcut-binding"><label for="shortcut-${action}">${esc(SCAN_SHORTCUT_ACTIONS[action])}<input id="shortcut-${action}" class="control" data-scan-shortcut-capture="${action}" data-managed="true" readonly autocomplete="off" aria-label="${esc(SCAN_SHORTCUT_ACTIONS[action])} shortcut" value="${esc(normalized.bindings[action]||'')}" placeholder="Unassigned"></label><button type="button" class="btn" data-scan-shortcut-unassign="${action}"${normalized.bindings[action]?'':' disabled'}>Unassign</button></div>`).join('')}</div>
 <button type="button" class="btn" data-scan-shortcuts-reset>Reset shortcuts</button>
 </details>
 <p class="scan-shortcut-error" data-scan-shortcut-error role="status" aria-live="polite"${error?'':' hidden'}>${esc(error)}</p>`;
}
