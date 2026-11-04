// Exercises the same web renderer embedded by the desktop client. Uses a
// disposable source copy so Next never rewrites a running developer's cache.
import assert from 'node:assert/strict';
import {test} from 'node:test';
import {spawn} from 'node:child_process';
import {createServer} from 'node:http';
import {once} from 'node:events';
import {mkdtemp,cp,symlink,rm,readFile,mkdir,readdir,appendFile} from 'node:fs/promises';
import {existsSync} from 'node:fs';
import {tmpdir} from 'node:os';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {chromium} from 'playwright';

const repo=fileURLToPath(new URL('../../',import.meta.url));
const binary=process.env.NATIVE_TEST_BINARY??path.join(repo,'target/debug/deeptutor-server'+(process.platform==='win32'?'.exe':''));
test('desktop renderer imports batches, hands files to test chat, and accepts dropped files',{skip:!existsSync(binary),timeout:360000},async()=>{
  const testFolder=path.join(repo,'web/tmp');await mkdir(testFolder,{recursive:true});
  const root=await mkdtemp(path.join(testFolder,'browser-files-ui-'));
  const children=[];let browser,page,logs='';
  const port=async()=>{const server=createServer();server.listen(0,'127.0.0.1');await once(server,'listening');const p=server.address().port;await new Promise(resolve=>server.close(resolve));return p};
  const start=(command,args,options={})=>{const child=spawn(command,args,{stdio:['ignore','pipe','pipe'],detached:process.platform!=='win32',...options});children.push(child);child.stdout.on('data',b=>{logs+=b});child.stderr.on('data',b=>{logs+=b});return child};
  const until=async fn=>{const deadline=Date.now()+90000;while(Date.now()<deadline){if(logs.includes('FATAL')||children.some(child=>child.exitCode!==null))throw new Error(logs.slice(-12000));try{const v=await fn();if(v)return v}catch{}await new Promise(resolve=>setTimeout(resolve,200))}throw new Error(logs.slice(-12000))};
  try {
    const backendPort=await port();const backend=`http://127.0.0.1:${backendPort}`;
    start(binary,['--home',path.join(root,'home'),'serve','--port',String(backendPort)],{env:{...process.env,DEEPTUTOR_AUTH_ENABLED:'false'}});
    await until(async()=> (await fetch(backend+'/health')).ok);
    const web=path.join(root,'web');const originalWeb=path.join(repo,'web');
    const ignored=new Set(['node_modules','public','out','dist','tmp','coverage','playwright-report','test-results']);
    await mkdir(web,{recursive:true});
    for(const name of await readdir(originalWeb))if(!ignored.has(name)&&!name.startsWith('.next')&&!name.startsWith('.env'))await cp(path.join(originalWeb,name),path.join(web,name),{recursive:true});
    await symlink(path.join(originalWeb,'node_modules'),path.join(web,'node_modules'),'dir');
    await symlink(path.join(originalWeb,'public'),path.join(web,'public'),'dir');
    // Keep dependencies and the disposable source inside one Turbopack root.
    await appendFile(path.join(web,'next.config.js'),`\nmodule.exports.outputFileTracingRoot=${JSON.stringify(repo)};module.exports.turbopack={...module.exports.turbopack,root:${JSON.stringify(repo)}};\n`);
    const frontendPort=await port();const frontend=`http://127.0.0.1:${frontendPort}`;
    start(process.execPath,[path.join(web,'node_modules/next/dist/bin/next'),'dev','--hostname','127.0.0.1','--port',String(frontendPort)],{cwd:web,env:{...process.env,DEEPTUTOR_API_BASE_URL:backend,NEXT_PUBLIC_API_BASE_EXTERNAL:backend,DEEPTUTOR_AUTH_ENABLED:'false',NEXT_PUBLIC_AUTH_ENABLED:'false',NEXT_TELEMETRY_DISABLED:'1',NODE_OPTIONS:'--max-old-space-size=4096'}});
    await until(async()=> (await fetch(frontend+'/api/health')).ok);
    browser=await chromium.launch({headless:true,...(!existsSync(chromium.executablePath())?{channel:'chrome'}:{})});
    const context=await browser.newContext({acceptDownloads:true,viewport:{width:1360,height:920}});
    page=await context.newPage();
    page.on('pageerror',error=>{logs+='\nBrowser: '+error.message});
    await page.goto(frontend+'/browser-testing',{waitUntil:'domcontentloaded',timeout:90000});
    const importer=page.getByRole('button',{name:/^(Import test files|导入测试文件)$/});
    await importer.waitFor({timeout:60000});
    const fixture=await readFile(path.join(repo,'rust/server/tests/fixtures/browser-cases.xlsx'));
    // A real native file input is used by Electron's file chooser as well.
    await page.locator('input[type=file]').setInputFiles([{name:'cases.xlsx',mimeType:'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',buffer:fixture},{name:'requirements.txt',mimeType:'text/plain',buffer:Buffer.from('Test https://example.com')}]);
    await page.getByText('cases.xlsx',{exact:false}).first().waitFor({timeout:30000});
    await page.getByText('requirements.txt',{exact:false}).first().waitFor();
    const sourceState=await (await fetch(backend+'/api/browser-testing')).json();assert.equal(sourceState.sources.length,2);
    const downloadEvent=page.waitForEvent('download');
    await page.getByRole('button',{name:/^(Download original|下载原文件)$/}).first().click();
    const download=await downloadEvent;assert.ok(download.suggestedFilename());
    await page.locator('li').filter({hasText:'cases.xlsx'}).getByRole('link',{name:/^(Test from this file|根据此文件测试)$/}).click();
    const textarea=page.getByPlaceholder(/Provide a website URL and describe|提供网站地址，告诉我/);
    await textarea.waitFor({timeout:180000});
    await until(async()=> (await textarea.inputValue()).includes('cases.xlsx'));
    await page.getByRole('button',{name:/^(Import test files|导入测试文件)$/}).waitFor();
    await page.getByLabel(/^(Import test files|导入测试文件)$/).setInputFiles({name:'second.txt',mimeType:'text/plain',buffer:Buffer.from('More acceptance criteria')});
    await until(async()=> (await textarea.inputValue()).includes('second.txt'));
    assert.doesNotMatch(await textarea.inputValue(),/source_id=src_/);
    const transfer=await page.evaluateHandle(()=>{const transfer=new DataTransfer();transfer.items.add(new File(['Dropped requirement'],'dropped.txt',{type:'text/plain'}));return transfer});
    await textarea.dispatchEvent('drop',{dataTransfer:transfer});
    await until(async()=> (await textarea.inputValue()).includes('dropped.txt'));
    const artifacts=path.join(repo,'dist/browser-testing-qa');await mkdir(artifacts,{recursive:true});
    await page.screenshot({path:path.join(artifacts,'desktop-test-files.png'),fullPage:true});
    console.log('Renderer screenshot:',path.join(artifacts,'desktop-test-files.png'));
  } catch(error) {
    if(page){const artifacts=path.join(repo,'dist/browser-testing-qa');await mkdir(artifacts,{recursive:true});await page.screenshot({path:path.join(artifacts,'desktop-test-files-failed.png'),timeout:5000}).catch(()=>{});console.log('Page URL:',page.url());console.log((await page.locator('body').innerText({timeout:5000}).catch(()=>'' )).slice(-6000));}
    console.log(logs.slice(-6000));throw error;
  } finally {
    await browser?.close();
    for(const child of children.reverse())if(child.exitCode===null){
      const done=once(child,'exit');
      const kill=signal=>{try{if(process.platform==='win32')child.kill(signal);else process.kill(-child.pid,signal)}catch{}};
      kill('SIGTERM');const timer=setTimeout(()=>kill('SIGKILL'),5000);await done;clearTimeout(timer);
    }
    if(process.env.KEEP_BROWSER_UI_QA!=='1')await rm(root,{recursive:true,force:true});
  }
});
