/** Inline notifications with bounded lifetime and explicit clipboard gestures. */
export function createNotice(node,{schedule=setTimeout,cancel=clearTimeout,copy=text=>navigator.clipboard.writeText(text)}={}){
  let timer=null;
  function hide(){if(timer!==null)cancel(timer);timer=null;node.hidden=true;}
  function show(message,isError=false){hide();node.textContent=String(message);node.className=isError?'notice error':'notice';node.hidden=false;timer=schedule(hide,5000);}
  node.addEventListener('click',hide);
  node.addEventListener('contextmenu',async event=>{event.preventDefault();const text=node.textContent;try{await copy(text);}catch(cause){show('Could not copy message: '+(cause.message||String(cause)),true);}});
  return {show,hide};
}
