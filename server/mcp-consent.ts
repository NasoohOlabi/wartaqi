export function mcpConsentPage(request: Request): Response {
  const nonce = crypto.randomUUID().replaceAll('-', '')
  const consent = new URL(request.url).pathname === '/mcp/consent'
  const html = `<!doctype html>
<html lang="ar" dir="rtl"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>وارتق · ربط تطبيق</title>
<style nonce="${nonce}">
:root{font-family:system-ui,sans-serif;color:#183529;background:#f5f7f2}*{box-sizing:border-box}body{margin:0;padding:24px 16px}main{max-width:560px;margin:5vh auto;background:white;border:1px solid #d7e1d8;border-radius:18px;padding:clamp(20px,5vw,36px);box-shadow:0 12px 40px #18352912}header{display:flex;justify-content:space-between;align-items:center;gap:12px}h1{font-size:1.55rem;line-height:1.25}p,li{line-height:1.55}button,input,select{font:inherit}button{min-height:44px;border-radius:9px;padding:10px 16px;cursor:pointer;border:1px solid #245b43;background:#245b43;color:white}button.secondary{background:white;color:#245b43}button:disabled{opacity:.6;cursor:wait}input,select{width:100%;min-height:44px;border:1px solid #94a99a;border-radius:8px;padding:10px;background:white;color:#183529}label{display:block;font-weight:600;margin:16px 0 6px}.actions{display:flex;gap:10px;flex-wrap:wrap;margin-top:20px}.actions button{flex:1}.account{background:#edf4ee;border-radius:10px;padding:12px;overflow-wrap:anywhere}.warning{border-inline-start:4px solid #b36a1c;padding-inline-start:14px}.error{color:#a12424;min-height:1.5em}small{color:#526a5a} [hidden]{display:none!important}
#client{overflow-wrap:anywhere}
</style></head><body><main><header><strong>وارتق · Wartaq</strong><button class="secondary" id="language" type="button" lang="en">English</button></header>
<h1 data-en="Connect an application" data-ar="ربط تطبيق">ربط تطبيق</h1>
<p id="client"></p><div id="current" class="account" hidden></div>
<section id="permission" hidden><p id="permissions" class="warning"></p><p data-en="Access works only after your account is approved and any required setup is complete." data-ar="يعمل الوصول فقط بعد الموافقة على حسابك وإكمال أي إعداد مطلوب.">يعمل الوصول فقط بعد الموافقة على حسابك وإكمال أي إعداد مطلوب.</p><div class="actions"><button id="accept" type="button" data-en="Allow access" data-ar="السماح بالوصول">السماح بالوصول</button><button id="decline" class="secondary" type="button" data-en="Decline" data-ar="رفض">رفض</button></div></section>
<section id="continue" hidden><p data-en="Continue with this account, or choose another account below." data-ar="تابع بهذا الحساب، أو اختر حساباً آخر أدناه.">تابع بهذا الحساب، أو اختر حساباً آخر أدناه.</p><button id="resume" type="button" data-en="Continue" data-ar="متابعة">متابعة</button></section>
<p id="setup" hidden><a href="/?page=app" target="_blank" rel="noopener noreferrer" data-en="Finish account setup in Wartaq" data-ar="أكمل إعداد الحساب في وارتق">أكمل إعداد الحساب في وارتق</a></p>
<section id="switch" hidden><label for="accounts" data-en="Switch account" data-ar="تبديل الحساب">تبديل الحساب</label><select id="accounts"></select><button id="switch-button" class="secondary" type="button" data-en="Use selected account" data-ar="استخدام الحساب المحدد">استخدام الحساب المحدد</button></section>
<details id="login">
<summary id="sign-in-title" data-en="Sign in to another account" data-ar="تسجيل الدخول إلى حساب آخر">تسجيل الدخول إلى حساب آخر</summary>
<form id="sign-in-form" method="post" aria-labelledby="sign-in-title">
<label for="sign-in-phone" data-en="Phone number" data-ar="رقم الهاتف">رقم الهاتف</label><input id="sign-in-phone" name="username" type="tel" dir="ltr" aria-describedby="phone-hint" autocomplete="username" enterkeyhint="next" required maxlength="40" placeholder="09xx xxx xxx / +963 9xx xxx xxx">
<small id="phone-hint" data-en="Syrian numbers can start with 09 or +9639." data-ar="يمكن أن يبدأ الرقم السوري بـ 09 أو +9639.">يمكن أن يبدأ الرقم السوري بـ 09 أو +9639.</small>
<label for="current-password" data-en="Password" data-ar="كلمة المرور">كلمة المرور</label><input id="current-password" name="password" type="password" dir="ltr" autocomplete="current-password" enterkeyhint="go" required maxlength="128">
<div class="actions"><button type="submit" data-en="Sign in" data-ar="تسجيل الدخول">تسجيل الدخول</button></div>
</form></details><p id="error" class="error" role="alert"></p><small data-en="Only approve applications you trust." data-ar="وافق فقط على التطبيقات التي تثق بها.">وافق فقط على التطبيقات التي تثق بها.</small></main>
<script nonce="${nonce}">
const isConsent = ${consent};
const query = location.search.slice(1);
const byId = id => document.getElementById(id);
let arabic = true;
try{arabic=localStorage.getItem('halaqa-language-preference')!=='en'}catch{}
let activeId = '';
const tr = (en, ar) => arabic ? ar : en;
function language(){document.documentElement.lang=arabic?'ar':'en';document.documentElement.dir=arabic?'rtl':'ltr';document.querySelectorAll('[data-en]').forEach(el=>{el.textContent=arabic?el.dataset.ar:el.dataset.en});document.title=tr('Wartaq · Connect an application','وارتق · ربط تطبيق');byId('language').lang=arabic?'en':'ar';byId('language').textContent=arabic?'English':'العربية';showClient();showPermissions()}
function showClient(){const client=new URLSearchParams(query).get('client_id') || '';byId('client').textContent=tr('Application ID: ','معرّف التطبيق: ')+client}
function showPermissions(){const scopes=new URLSearchParams(query).get('scope')?.split(' ') || [];const lines=[];if(scopes.includes('mcp:read'))lines.push(tr('Read data available to your account, including sensitive student details, attendance and notes.','قراءة البيانات المتاحة لحسابك، بما فيها بيانات الطلاب الحساسة والحضور والملاحظات.'));if(scopes.includes('mcp:write'))lines.push(tr('Change data available to your account. Changes may be permanent.','تغيير البيانات المتاحة لحسابك. قد تكون التغييرات دائمة.'));if(scopes.includes('mcp:read')&&scopes.includes('mcp:write'))lines.push(tr('Administrators may also grant access, manage accounts and reset passwords.','يمكن للمسؤولين أيضاً منح الوصول وإدارة الحسابات وإعادة تعيين كلمات المرور.'));if(scopes.includes('offline_access'))lines.push(tr('Maintain access while your session remains active.','الاحتفاظ بالوصول ما دامت جلستك نشطة.'));byId('permissions').textContent=lines.join(' ') || tr('No notebook permissions requested.','لم تُطلب صلاحيات للدفتر.')}
byId('language').onclick=()=>{arabic=!arabic;try{localStorage.setItem('halaqa-language-preference',arabic?'ar':'en')}catch{}language()};language();
async function post(path, body, userId){const headers={'content-type':'application/json'};if(userId)headers['x-user-id']=userId;const response=await fetch(path,{method:'POST',headers,body:JSON.stringify(body),credentials:'same-origin'});if(!response.ok)throw new Error('request');return response.json()}
function redirect(result){if(!result || typeof result.url!=='string')throw new Error('redirect');const url=new URL(result.url,location.origin);if(['javascript:','data:','vbscript:'].includes(url.protocol))throw new Error('redirect');location.assign(url.href)}
async function run(action){byId('error').textContent='';document.querySelectorAll('button').forEach(b=>b.disabled=true);try{await action()}catch{byId('error').textContent=tr('Could not complete the request. Check your details and try again.','تعذر إكمال الطلب. تحقق من البيانات وحاول مرة أخرى.')}finally{document.querySelectorAll('button').forEach(b=>b.disabled=false)}}
async function load(){const [sessionResponse,accountsResponse]=await Promise.all([fetch('/api/auth/get-session',{cache:'no-store'}),fetch('/api/device-accounts',{cache:'no-store'})]);if(!sessionResponse.ok||!accountsResponse.ok)throw new Error('session');const session=await sessionResponse.json();const saved=await accountsResponse.json();activeId=session?.user?.id || '';if(activeId){byId('current').hidden=false;byId('current').textContent=tr('Signed in as: ','تم تسجيل الدخول باسم: ')+(session.user.name || '');byId(isConsent?'permission':'continue').hidden=false;const accountResponse=await fetch('/api/me',{cache:'no-store',headers:{'x-user-id':activeId}});if(accountResponse.ok){const account=await accountResponse.json();byId('setup').hidden=!account?.account?.invitationProfile?.requiresSetup}}else{byId('login').open=true}const accounts=Array.isArray(saved.accounts)?saved.accounts.filter(a=>a.userId!==activeId):[];if(accounts.length){byId('switch').hidden=false;for(const account of accounts){const option=document.createElement('option');option.value=account.userId;option.textContent=account.name+' · '+account.identifier;byId('accounts').append(option)}}}
const resume=()=>post('/api/auth/oauth2/continue',{oauth_query:query,selected:true});
byId('sign-in-form').onsubmit=e=>{e.preventDefault();const form=e.currentTarget;const credentials=new FormData(form);run(async()=>{const body={phone:credentials.get('username'),password:credentials.get('password'),oauth_query:query};const result=await post('/api/auth/sign-in/phone',body);form.reset();redirect(result?.url ? result : await resume())})};
byId('switch-button').onclick=()=>run(async()=>{await post('/api/device-accounts/switch',{userId:byId('accounts').value});location.reload()});
byId('resume').onclick=()=>run(async()=>redirect(await resume()));
for(const [id,accept] of [['accept',true],['decline',false]])byId(id).onclick=()=>run(async()=>redirect(await post('/api/auth/oauth2/consent',{oauth_query:query,accept},activeId)));
run(load);
</script></body></html>`
  return new Response(html, {
    headers: {
      'Content-Type': 'text/html; charset=utf-8',
      'Cache-Control': 'no-store',
      'Referrer-Policy': 'no-referrer',
      'X-Content-Type-Options': 'nosniff',
      'Content-Security-Policy': `default-src 'none'; script-src 'nonce-${nonce}'; style-src 'nonce-${nonce}'; connect-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'`,
    },
  })
}
