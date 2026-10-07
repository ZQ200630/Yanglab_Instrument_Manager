/** The production API reaches instruments only through their owning Rust Host. */
import {createHostClient} from './host-client.js';
export {connectLocalHost} from './host-client.js';
export function createClient(invoke,listen){return createHostClient(invoke,listen);}
export function tauriClient(){
 const native=globalThis.__TAURI__;
 if(!native?.core?.invoke||!native?.event?.listen)throw new Error('Open this console in the desktop App');
 return createClient(native.core.invoke,native.event.listen);
}
