// Use a disposable backend home and a separate frontend before running:
// AUTH_QA_WEB_URL=http://127.0.0.1:3799 AUTH_QA_ALLOW_SETUP=1 node scripts/native-auth-acceptance.mjs
import assert from 'node:assert/strict';
import {mkdir} from 'node:fs/promises';
import {chromium} from '../packages/browser-testing/node_modules/playwright/index.mjs';

const base = process.env.AUTH_QA_WEB_URL;
if (!base || process.env.AUTH_QA_ALLOW_SETUP !== '1') throw new Error('Point AUTH_QA_WEB_URL at a disposable test deployment and set AUTH_QA_ALLOW_SETUP=1.');
const artifacts = new URL('../dist/auth-acceptance/', import.meta.url).pathname;
await mkdir(artifacts, {recursive:true});
const browser = await chromium.launch({headless:true, channel:process.env.AUTH_QA_BROWSER_CHANNEL || 'chrome'});
const admin = await browser.newContext({viewport:{width:1280,height:900}});
const ordinary = await browser.newContext({viewport:{width:1280,height:900}});
admin.setDefaultTimeout(90000);
ordinary.setDefaultTimeout(90000);
admin.setDefaultNavigationTimeout(120000);
ordinary.setDefaultNavigationTimeout(120000);
const page = await admin.newPage();
const errors=[];
page.on('pageerror',error=>errors.push(error.message));
try {
  await page.goto(`${base}/admin/users`);
  const status = await (await admin.request.get(`${base}/api/auth/status`)).json();
  if (!status.enabled) {
    await page.getByRole('heading',{name:/Enable multi-user mode|开启多用户模式/}).waitFor({timeout:60000});
    await page.screenshot({path:`${artifacts}setup.png`,fullPage:true});
    await page.getByLabel(/Username|用户名/,{exact:true}).fill('qa-admin');
    await page.getByLabel(/Password|密码/,{exact:true}).fill('qa-admin-password');
    await page.getByRole('button',{name:/Enable multi-user mode|开启多用户模式/}).click();
  } else if (!status.authenticated) {
    assert.equal((await admin.request.post(`${base}/api/auth/login`,{data:{username:'qa-admin',password:'qa-admin-password'}})).status(),200);
    await page.goto(`${base}/admin/users`);
  }
  await page.getByRole('heading',{name:/User Management|用户管理/}).waitFor();
  const users=await (await admin.request.get(`${base}/api/auth/users`)).json();
  if (users.some(user=>user.username==='qa-user')) {
    assert.equal((await admin.request.delete(`${base}/api/auth/users/qa-user`)).status(),200);
    await page.reload();
  }
  const catalog={version:1,connections:[],services:{llm:{active_profile_id:'qa-provider',active_model_id:'qa-model',profiles:[{id:'qa-provider',name:'QA provider',binding:'custom',base_url:'http://127.0.0.1:9/v1',api_key:'synthetic-ui-secret',models:[{id:'qa-model',name:'QA model',model:'qa-model'}]}]}}};
  assert.equal((await admin.request.put(`${base}/api/settings/catalog`,{data:{catalog}})).status(),200);
  await page.getByRole('button',{name:/Add user|添加用户/}).first().click();
  const dialog=page.getByRole('dialog');
  await dialog.getByLabel(/Username \(or email\)|用户名（或邮箱）/).fill('qa-user');
  await dialog.getByLabel(/Password \(≥ 8 chars\)|密码/).fill('qa-user-password');
  await dialog.screenshot({path:`${artifacts}add-user.png`});
  await dialog.getByRole('button',{name:/^Create$|^创建$/}).click();
  const row=page.getByRole('row').filter({hasText:'qa-user'});
  await row.waitFor();
  assert.match(await row.innerText(),/User|用户/);
  await row.getByRole('button',{name:/Manage assignments|管理分配|管理授权/}).click();
  await page.getByLabel('QA model',{exact:true}).check();
  const saving=page.waitForResponse(response=>response.url().includes('/grants') && response.request().method()==='PUT');
  await page.getByRole('button',{name:/Save assignments|保存分配|保存授权/}).click();
  const saved=await saving;
  assert.equal(saved.status(),200,await saved.text());
  await page.getByRole('button',{name:/^Saved$|^已保存$/}).waitFor();
  await page.screenshot({path:`${artifacts}users-desktop.png`,fullPage:true});
  await page.setViewportSize({width:390,height:844});
  await page.getByRole('button',{name:/Add user|添加用户/}).first().scrollIntoViewIfNeeded();
  await page.screenshot({path:`${artifacts}users-mobile.png`,fullPage:true});

  const user=await ordinary.newPage();
  await user.goto(`${base}/admin/users`,{waitUntil:'domcontentloaded'});
  await user.waitForURL(/\/login/);
  await user.getByLabel(/Email or username|邮箱或用户名/).fill('qa-user');
  await user.getByLabel(/Password|密码/,{exact:true}).fill('qa-user-password');
  await user.getByRole('button',{name:/Sign in|登录/}).click();
  await user.waitForURL(/\/profile\?change=1/);
  await user.getByLabel(/Current password|当前密码/,{exact:true}).fill('qa-user-password');
  await user.getByLabel(/New password|新密码/,{exact:true}).fill('qa-owned-password');
  await user.getByLabel(/Confirm new password|确认新密码/,{exact:true}).fill('qa-owned-password');
  await user.getByRole('button',{name:/Update password|更新密码/}).click();
  await user.getByText(/Password updated|密码已更新/,{exact:true}).waitFor();
  const models=user.locator('#models');
  await models.getByRole('button',{name:/Add personal model|添加个人模型/}).click();
  await models.getByLabel(/^Name$|^名称$/).fill('QA personal provider');
  await models.getByLabel(/Base URL|基础 URL|基础地址/).fill('http://127.0.0.1:9/v1');
  await models.getByLabel(/Model identifier|模型标识/).fill('qa-personal-model');
  await models.getByLabel(/Token \/ API key|Token \/ API Key/).fill('synthetic-personal-ui-key');
  await models.getByRole('button',{name:/Save personal models|保存个人模型/}).click();
  await models.getByText(/Personal models saved|个人模型已保存/).waitFor();
  await models.screenshot({path:`${artifacts}personal-and-assigned-models.png`});
  assert.ok((await models.innerText()).includes('QA model'));
  assert.equal((await ordinary.request.get(`${base}/api/auth/users`)).status(),403);
  assert.equal((await ordinary.request.put(`${base}/api/settings/catalog`,{data:{catalog}})).status(),403);
  const options=await (await ordinary.request.get(`${base}/api/settings/llm-options`)).json();
  assert.equal(options.options.length,2);
  assert.ok(options.options.some(model=>model.model==='qa-personal-model' && model.source==='personal'));
  assert.ok(options.options.some(model=>model.model_id==='qa-model' && model.source==='admin'));
  const own=await (await ordinary.request.get(`${base}/api/auth/profile/models`)).text();
  assert.ok(!own.includes('synthetic-personal-ui-key') && !own.includes('synthetic-ui-secret'));
  const visible=await (await ordinary.request.get(`${base}/api/settings/catalog`)).text();
  assert.ok(!visible.includes('synthetic-ui-secret'));
  await user.goto(`${base}/admin/users`,{waitUntil:'domcontentloaded'}).catch(error=>{if(!String(error).includes('ERR_ABORTED'))throw error;});
  await user.waitForURL(url=>url.pathname==='/' || url.pathname==='/chat');
  assert.equal(await user.getByRole('link',{name:/User Management|用户管理/}).count(),0);
  await ordinary.request.post(`${base}/api/auth/logout`,{data:{}});
  await user.goto(`${base}/admin/users`);
  await user.waitForURL(/\/login/);
  assert.equal(errors.length,0,errors.join('\n'));
  console.log(JSON.stringify({passed:true,checks:['local setup','administrator creation form','model assignment','mobile entry','anonymous redirect','temporary password change','personal API key configuration','personal and assigned models together','ordinary-user authorization','secret redaction','logout redirect'],artifacts},null,2));
} catch(error) {
  console.error(error);
  console.error((await page.locator('body').innerText()).slice(-5000));
  await page.screenshot({path:`${artifacts}failure.png`,fullPage:true,timeout:10000}).catch(()=>{});
  throw error;
} finally {await browser.close();}
