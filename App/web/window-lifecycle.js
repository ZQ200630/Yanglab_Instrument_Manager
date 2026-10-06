export function closeDecision(report){return {...report,windowMayClose:report?.released===true&&(report.cleanup_attempts||[]).every(a=>a.confirmed===true)};}
export async function closeClient(client){return closeDecision(await client.closeClient());}
