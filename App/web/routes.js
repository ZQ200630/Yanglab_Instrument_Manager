const id=value=>typeof value==='string'&&/^[0-9a-f]{32}$/.test(value);
export function deviceKey(hostId,domain){
  if(!id(hostId)||!id(domain?.id)||!['device','setup'].includes(domain.kind))throw new TypeError('Invalid instance identity');
  return `${hostId}/${domain.kind}/${domain.id}`;
}
export function routeFor(hostId,domain){return `#/host/${deviceKey(hostId,domain)}`;}
export function parseRoute(value){
  const match=/^#\/host\/([0-9a-f]{32})\/(device|setup)\/([0-9a-f]{32})$/.exec(value||'');
  return match?{hostId:match[1],domain:{kind:match[2],id:match[3]}}:null;
}
