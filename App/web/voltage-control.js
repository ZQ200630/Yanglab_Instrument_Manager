import {formatDigits} from './wavelength-editor.js';
export const voltageDraftIds=Array.from({length:8},(_,index)=>`voltage-${index+1}`);
// Drafts belong to one connection. A safety epoch does not submit or discard an edit.
export function syncVoltageDraft(local,domain,bootId){
 const binding=JSON.stringify([bootId,domain?.context?.session_id,domain?.context?.connection_id]);
 const changed=local.voltageDraftBinding!==binding;
 local.inputs??=new Map();local.voltageDraftDirty??=new Set();
 if(changed){for(const id of voltageDraftIds)local.inputs.delete(id);local.voltageDraftDirty.clear();local.voltageHistory=[];local.voltageDraftBinding=binding;}
 if(!domain?.context?.connection_id)return changed;
 for(const [index,id]of voltageDraftIds.entries()){
  const value=domain.device?.requested_voltage_v?.[index];
  if(!local.voltageDraftDirty.has(id)&&Number.isFinite(value)&&value>=0&&value<=14)
   local.inputs.set(id,{value:formatDigits(value,3,2)});
 }
 return changed;
}
