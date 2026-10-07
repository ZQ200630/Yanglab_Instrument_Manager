/** Background events share one paint; user actions can flush their feedback now. */
export function createRenderScheduler(paint,schedule=callback=>requestAnimationFrame(callback),unschedule=id=>cancelAnimationFrame(id)){
 let frame=null;
 const cancel=()=>{if(frame!==null){unschedule(frame);frame=null;}};
 return Object.freeze({request(){if(frame===null)frame=schedule(()=>{frame=null;paint()});},flush(){cancel();paint();},cancel});
}

function identity(node){
 if(node.nodeType!==1)return String(node.nodeType);
 const key=node.id||['ui','op','device','setup','side','channel','setting','command','draft','param','pmTab','archive','name'].map(k=>node.dataset?.[k]||'').join('|');
 return [node.namespaceURI,node.nodeName,key,node.id||node.hasAttribute('data-ui')||node.hasAttribute('data-op')?'':node.getAttribute('class')||''].join(':');
}
function update(current,next){
 if(current.nodeType!==1){if(current.nodeValue!==next.nodeValue)current.nodeValue=next.nodeValue;return;}
 for(const attr of [...current.attributes]){
  if(current.nodeName==='DETAILS'&&attr.name==='open')continue;
  if(!next.hasAttribute(attr.name))current.removeAttribute(attr.name);
 }
 for(const attr of next.attributes){
  if(current.nodeName==='DETAILS'&&attr.name==='open')continue;
  if(current.getAttribute(attr.name)!==attr.value)current.setAttribute(attr.name,attr.value);
 }
 children(current,next);
 // Native controls keep dirty properties independently of their HTML attributes.
 // Managed values are current authority; ordinary edits are preserved by the caller.
 if(current.hasAttribute('data-managed')){
  if(current.nodeName==='INPUT'&&['checkbox','radio'].includes(current.type))current.checked=next.checked;
  else if(['INPUT','SELECT','TEXTAREA'].includes(current.nodeName)&&current.value!==next.value)current.value=next.value;
 }
}
function children(current,next){
 let cursor=current.firstChild;
 for(const incoming of [...next.childNodes]){
  let match=cursor;
  if(incoming.nodeType!==1){if(!match||identity(match)!==identity(incoming))match=null;}
  else while(match&&identity(match)!==identity(incoming))match=match.nextSibling;
  if(match){
   // Remove obsolete predecessors rather than reinsert the live matching subtree.
   while(cursor!==match){const obsolete=cursor;cursor=cursor.nextSibling;current.removeChild(obsolete);}
   update(match,incoming);cursor=match.nextSibling;
  }else current.insertBefore(incoming.cloneNode(true),cursor);
 }
 while(cursor){const obsolete=cursor;cursor=cursor.nextSibling;current.removeChild(obsolete);}
}
/** Reuse compatible live controls on the same page, including native select popups. */
export function replaceMarkup(target,previous,next){
 if(previous===next)return false;
 if(previous===null||!target.ownerDocument?.createElement){target.innerHTML=next;return true;}
 const template=target.ownerDocument.createElement('template');template.innerHTML=next;
 children(target,template.content);return true;
}
