// Render the production OSA panel without a Host or instrument connection.
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {readFile} from 'node:fs/promises';
import {fileURLToPath,pathToFileURL} from 'node:url';
import {resolve,sep} from 'node:path';

const root=resolve(fileURLToPath(new URL('../../..',import.meta.url)));
const {chromium}=await import(pathToFileURL(process.env.YANG_LAB_PLAYWRIGHT_MODULE).href);
const html=`<!doctype html><link rel="stylesheet" href="/App/web/style.css">
<div class="app-shell"><aside class="sidebar">Yang LAB</aside><div class="main-shell"><main id="content" class="content"></main></div></div>
<script type="module">
import {osa} from '/App/web/panels.js';
document.getElementById('content').innerHTML=osa({
  status:{devices:{osa:{connected:false}}},
  recordingRoot:'\\\\?\\C:\\Users\\PIC_YangLab\\AppData\\Local\\Packages\\OpenAI.Codex_2p2nqsd0c76g0\\LocalCache\\Local\\Yang LAB INSTRUMENT CONSOLE Native\\Result'
});
window.ready=true;
</script>`;
const server=createServer(async(req,res)=>{
  try {
    if(req.url==='/'){res.setHeader('Content-Type','text/html');res.end(html);return;}
    const path=resolve(root,'.'+new URL(req.url,'http://localhost').pathname);
    if(!path.startsWith(resolve(root,'App/web')+sep)){res.writeHead(404).end();return;}
    res.setHeader('Content-Type',path.endsWith('.js')?'text/javascript':'text/css');
    res.end(await readFile(path));
  }catch{res.writeHead(404).end();}
});
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
let browser;
try {
  browser=await chromium.launch({executablePath:process.env.YANG_LAB_BROWSER,headless:true,timeout:15000});
  const page=await browser.newPage({viewport:{width:1440,height:900}});
  await page.goto('http://127.0.0.1:'+server.address().port);
  await page.waitForFunction(()=>window.ready);
  const layout=await page.evaluate(()=>({
    viewport:window.innerWidth,
    document:document.documentElement.scrollWidth,
    right:document.querySelector('.device-layout .stack .card').getBoundingClientRect().right
  }));
  assert.ok(layout.document<=layout.viewport,`OSA page must not scroll horizontally: ${JSON.stringify(layout)}`);
  assert.ok(layout.right<=layout.viewport,`Capture card must remain visible: ${JSON.stringify(layout)}`);
  console.log('PASS: OSA panel and capture card fit a 1440px installed-app window with a long save path');
}finally{
  await browser?.close();
  await new Promise(resolve=>server.close(resolve));
}
