// Browser check for sideloaded apps. From tests/e2e after `npm install`:
//   BASE=http://127.0.0.1:8090 OUT=/workspace/artcraft-cloud-shots ARTCRAFT_DATA=$ARTCRAFT_DATA node sideload.js
// The server can run with only ARTCRAFT_PUBLIC_URL (same-host sandbox is the default).
// Super-admin login is admin / admin-pass-12345 only when that is how the test server was started.

const puppeteer = require('puppeteer-core');
const fs = require('fs');
const http = require('http');
const path = require('path');
const { execSync } = require('child_process');
const sleep = ms => new Promise(r => setTimeout(r, ms));
const BASE = process.env.BASE || 'http://127.0.0.1:8090';
const OUT = process.env.OUT || '/workspace/artcraft-cloud-shots';
const DATA = process.env.ARTCRAFT_DATA || '/workspace/acc-run/data';
const log = (...a) => console.log(new Date().toISOString().slice(11, 19), ...a);
fs.mkdirSync(OUT, { recursive: true });

function makeZip(id, version, dest) {
  const dir = fs.mkdtempSync('/tmp/sidecraft-');
  const html = `<!doctype html><title>SideCraft ${version}</title><script>
parent.postMessage({type:'${id}:ready'},'*') // host=parent
window.__probe=async()=>{
  const out={origin: String(location.origin)};
  try{const r=await fetch('/api/me',{credentials:'include',headers:{'x-requested-with':'artcraft'}});out.relative={status:r.status,body:await r.text()};}catch(e){out.relative={error:String(e)};}
  try{const r=await fetch(${JSON.stringify(BASE)}+'/api/me',{credentials:'include',headers:{'x-requested-with':'artcraft'}});out.absolute={status:r.status,body:await r.text()};}catch(e){out.absolute={error:String(e)};}
  try{out.parentTitle=parent.document.title; out.parentReached=true;}catch(e){out.parentError=String(e); out.parentReached=false;}
  return out;
};
window.__save=(name, text, saveAs)=>{
  const buf=new TextEncoder().encode(text).buffer;
  const origin=window.__ARTCRAFT_PARENT__||'*';
  parent.postMessage({type:'${id}:save',name,saveAs:!!saveAs,bytes:buf},origin);
};
</script>`;
  fs.writeFileSync(path.join(dir, 'index.html'), html);
  execSync(`python3 -c "import zipfile; z=zipfile.ZipFile('${dest}','w'); z.write('${path.join(dir,'index.html')}','index.html')"`);
  fs.rmSync(dir, { recursive: true });
}

function sha256(file) {
  return execSync(`sha256sum ${file}`).toString().split(' ')[0];
}

async function login(page) {
  await page.goto(BASE + '/', { waitUntil: 'networkidle0' });
  await page.evaluate(async () => {
    await fetch('/api/session', { method: 'POST', headers: { 'content-type': 'application/json', 'x-requested-with': 'artcraft' }, body: JSON.stringify({ username: 'admin', password: 'admin-pass-12345' }) });
  });
  await page.goto(BASE + '/', { waitUntil: 'networkidle0' });
}

function fileOnDisk(name) {
  return path.join(DATA, 'files/1', name);
}

(async () => {
  const zip1 = '/tmp/sidecraft-web-0.1.0.zip';
  const zip2 = '/tmp/sidecraft-web-0.2.0.zip';
  makeZip('sidecraft', '0.1.0', zip1);
  makeZip('sidecraft', '0.2.0', zip2);
  const sha2 = sha256(zip2);
  const files = { 'sidecraft-web-0.2.0.zip': fs.readFileSync(zip2) };
  files['sidecraft.json'] = Buffer.from(JSON.stringify({ id: 'sidecraft', versions: [{ version: '0.2.0', url: 'http://127.0.0.1:PORT/sidecraft-web-0.2.0.zip', sha256: sha2 }] }));
  const server = http.createServer((req, res) => {
    const name = req.url.replace(/^\//, '');
    if (files[name]) {
      res.writeHead(200, { 'content-type': name.endsWith('.json') ? 'application/json' : 'application/zip' });
      let body = files[name];
      if (name.endsWith('.json')) body = Buffer.from(body.toString().replace('PORT', String(server.address().port)));
      res.end(body);
    } else {
      res.writeHead(404); res.end('missing');
    }
  });
  await new Promise(r => server.listen(0, '127.0.0.1', r));
  const manifest = `http://127.0.0.1:${server.address().port}/sidecraft.json`;
  log('manifest', manifest);

  const browser = await puppeteer.launch({ executablePath: process.env.CHROME || '/usr/bin/google-chrome', headless: 'new', args: ['--no-sandbox', '--use-angle=swiftshader', '--enable-unsafe-swiftshader', '--window-size=1440,1000'] });
  const page = await browser.newPage();
  await page.setViewport({ width: 1440, height: 1000 });
  const errors = [];
  page.on('pageerror', e => errors.push('pageerror ' + e.message.slice(0, 300)));
  page.on('dialog', async d => { log('dialog', d.type(), d.message().slice(0, 200)); await d.dismiss(); });
  await login(page);

  const sessionCookie = (await page.cookies()).find(c => c.name === 'artcraft_session');
  if (!sessionCookie) throw new Error('no session cookie after login');
  if (!sessionCookie.httpOnly) throw new Error('session cookie is not HttpOnly');
  const sameSite = String(sessionCookie.sameSite || '').toLowerCase();
  if (sameSite && sameSite !== 'lax' && sameSite !== 'strict') throw new Error('session cookie SameSite=' + sessionCookie.sameSite);

  await page.goto(BASE + '/admin/apps', { waitUntil: 'networkidle0' });
  await page.waitForSelector('#danger-zone, .danger-zone, section.danger-zone');
  await page.evaluate(() => document.querySelector('.danger-zone, #danger-zone')?.scrollIntoView());
  await sleep(400);
  await page.screenshot({ path: path.join(OUT, 'sideload-v2-1-danger-zone.png'), fullPage: true });
  log('danger zone visible');

  const zipInput = await page.$('.sideload-form input[type=file]');
  if (!zipInput) throw new Error('no zip input');
  const stacks = await page.evaluate(() => document.querySelectorAll('.sideload-form label.stack').length);
  if (stacks < 6) throw new Error('sideload form is not using label.stack (' + stacks + ')');
  await page.evaluate(() => {
    const val = (label, value) => {
      const el = [...document.querySelectorAll('.sideload-form label')].find(l => l.textContent.trim().startsWith(label));
      const input = el && el.querySelector('input,select');
      if (!input) throw new Error('no field ' + label);
      input.focus();
      if (input.tagName === 'SELECT') { input.value = value; input.dispatchEvent(new Event('change', { bubbles: true })); }
      else { input.value = value; input.dispatchEvent(new Event('input', { bubbles: true })); }
    };
    val('App id', 'sidecraft');
    val('Name', 'SideCraft');
    val('Version', '0.1.0');
    val('Publisher', 'Test');
    val('File types', 'side');
  });
  await page.evaluate((url) => {
    const labels = [...document.querySelectorAll('.sideload-form label')];
    const kind = labels.find(l => l.querySelector('span')?.textContent.trim() === 'Source')?.querySelector('select');
    if (kind) { kind.value = 'manifest'; kind.dispatchEvent(new Event('change', { bubbles: true })); }
    const src = labels.find(l => l.textContent.includes('Source URL'))?.querySelector('input');
    if (src) { src.value = url; src.dispatchEvent(new Event('input', { bubbles: true })); }
  }, manifest);
  await zipInput.uploadFile(zip1);
  await page.evaluate(() => {
    const boxes = [...document.querySelectorAll('.sideload-form input[type=checkbox]')];
    const confirm = boxes.find(b => b.parentElement.textContent.includes('trust list'));
    if (confirm && !confirm.checked) { confirm.click(); }
  });
  await sleep(200);
  const isolationUi = await page.evaluate(() => {
    const legend = [...document.querySelectorAll('.sideload-form legend, .isolation-picker legend')].some(l => /Isolation/.test(l.textContent));
    const table = !!document.querySelector('.isolation-compare');
    const sandbox = [...document.querySelectorAll('.sideload-form input[type=radio]')].some(r => r.value === 'sandbox' && r.checked);
    const domainDisabled = [...document.querySelectorAll('.sideload-form input[type=radio]')].some(r => r.value === 'domain' && r.disabled);
    return { legend, table, sandbox, domainDisabled };
  });
  log('isolation ui', isolationUi);
  if (!isolationUi.legend || !isolationUi.table) throw new Error('isolation comparison missing from the form');
  if (!isolationUi.sandbox) throw new Error('sandbox is not the default isolation');
  if (!isolationUi.domainDisabled) throw new Error('separate domain should be disabled when ARTCRAFT_APPS_URL is unset');
  await page.screenshot({ path: path.join(OUT, 'sideload-v2-2-form-filled.png'), fullPage: true });
  await page.evaluate(() => document.querySelector('.sideload-form button[type=submit]').click());
  await page.waitForFunction(() => [...document.querySelectorAll('li.app-row strong')].some(s => s.textContent.includes('SideCraft')), { timeout: 60000 });
  await sleep(600);
  await page.screenshot({ path: path.join(OUT, 'sideload-v2-3-installed.png'), fullPage: true });
  const row = await page.evaluate(() => [...document.querySelectorAll('li.app-row')].find(r => r.textContent.includes('SideCraft'))?.innerText || '');
  log('installed row', row.replace(/\n/g, ' | '));
  if (!row.includes('Unverified')) throw new Error('missing Unverified badge');

  // Grants and file-types dialogs (design-system, no window.prompt).
  await page.evaluate(() => {
    const row = [...document.querySelectorAll('li.app-row')].find(r => r.textContent.includes('SideCraft'));
    const b = row && [...row.querySelectorAll('button')].find(b => b.textContent.trim() === 'Grants…');
    if (b) b.click();
  });
  await page.waitForFunction(() => [...document.querySelectorAll('.dialog h2, .dialog h1')].some(h => /Grants for/.test(h.textContent)), { timeout: 10000 });
  await sleep(300);
  await page.screenshot({ path: path.join(OUT, 'sideload-v2-3b-grants.png') });
  await page.evaluate(() => [...document.querySelectorAll('.dialog button')].find(b => b.textContent.trim() === 'Cancel')?.click());
  await sleep(200);
  await page.evaluate(() => {
    const row = [...document.querySelectorAll('li.app-row')].find(r => r.textContent.includes('SideCraft'));
    const b = row && [...row.querySelectorAll('button')].find(b => b.textContent.trim() === 'File types…');
    if (b) b.click();
  });
  await page.waitForFunction(() => [...document.querySelectorAll('.dialog h2, .dialog h1, .dialog label')].some(h => /File types/.test(h.textContent)), { timeout: 10000 });
  await sleep(300);
  await page.screenshot({ path: path.join(OUT, 'sideload-v2-3c-file-types.png') });
  await page.evaluate(() => [...document.querySelectorAll('.dialog button')].find(b => b.textContent.trim() === 'Cancel')?.click());
  await sleep(200);
  await page.evaluate(() => {
    const row = [...document.querySelectorAll('li.app-row')].find(r => r.textContent.includes('SideCraft'));
    const b = row && [...row.querySelectorAll('button')].find(b => b.textContent.trim() === 'Isolation…');
    if (b) b.click();
  });
  await page.waitForFunction(() => [...document.querySelectorAll('.dialog h2, .dialog h1')].some(h => /Isolation for/.test(h.textContent)), { timeout: 10000 });
  await sleep(300);
  await page.screenshot({ path: path.join(OUT, 'sideload-v2-3d-isolation.png') });
  await page.evaluate(() => [...document.querySelectorAll('.dialog button')].find(b => b.textContent.trim() === 'Cancel')?.click());
  await sleep(200);

  // Asset CSP on the same host; Origin: null must not use the session.
  const me = await page.evaluate(async () => (await fetch('/api/me', { headers: { 'x-requested-with': 'artcraft' } })).json());
  if (me.apps_origin) log('optional apps origin', me.apps_origin);
  const cookieHeader = (await page.cookies()).map(c => c.name + '=' + c.value).join('; ');
  const asset = await new Promise((resolve, reject) => {
    const u = new URL(BASE);
    const r = http.request({ hostname: u.hostname, port: u.port || 80, path: '/apps/sidecraft/index.html',
      headers: { Host: u.host, cookie: cookieHeader } },
      resp => { let body = ''; resp.on('data', d => body += d); resp.on('end', () => resolve({ status: resp.statusCode, csp: resp.headers['content-security-policy'] || '', body })); });
    r.on('error', reject); r.end();
  });
  log('sideload asset', asset.status, asset.csp);
  if (asset.status !== 200) throw new Error('expected sideloaded app on the main host, got ' + asset.status);
  if (!/sandbox/i.test(asset.csp) || !/allow-scripts/i.test(asset.csp) || /allow-same-origin/i.test(asset.csp)) {
    throw new Error('sideload CSP should sandbox allow-scripts without allow-same-origin: ' + asset.csp);
  }
  // Open a .side file with the sideloaded app.
  const filesDir = path.join(DATA, 'files/1');
  fs.mkdirSync(filesDir, { recursive: true });
  fs.writeFileSync(fileOnDisk('hello.side'), 'hello from sideload');
  await page.goto(BASE + '/', { waitUntil: 'networkidle0' });
  await page.waitForSelector('.file-table, .new-cards');
  await sleep(800);
  const hasTile = await page.evaluate(() => document.body.innerText.includes('SideCraft'));
  log('launcher has SideCraft', hasTile);
  await page.screenshot({ path: path.join(OUT, 'sideload-v2-4-launcher.png') });
  const opened = await page.evaluate(() => {
    const name = [...document.querySelectorAll('.file-table .file-name, .file-name')].find(b => b.textContent.trim() === 'hello.side');
    if (name) { name.click(); return true; }
    return false;
  });
  if (!opened) {
    await page.goto(BASE + '/open/hello.side', { waitUntil: 'networkidle0' });
  }
  await sleep(1500);
  const warning = await page.evaluate(() => document.querySelector('.dialog h2, .dialog p')?.textContent || '');
  log('first-open', warning);
  if (/Unverified|trust list|administrator/i.test(warning)) {
    await page.screenshot({ path: path.join(OUT, 'sideload-v2-5-first-open.png') });
    await page.evaluate(() => [...document.querySelectorAll('.dialog button')].find(b => /anyway|Open/i.test(b.textContent))?.click());
  }
  await page.waitForSelector('section.editor iframe', { timeout: 20000 });
  await sleep(2000);
  const frameInfo = await page.$eval('section.editor iframe', f => ({ src: f.getAttribute('src') || '', sandbox: f.getAttribute('sandbox') || '' }));
  log('iframe', frameInfo);
  if (/^https?:\/\/apps\./.test(frameInfo.src) && !me.apps_origin) throw new Error('iframe unexpectedly used a second host: ' + frameInfo.src);
  if (!frameInfo.src.includes('/apps/sidecraft')) throw new Error('iframe is not the sideloaded app: ' + frameInfo.src);
  if (!/(^|\s)allow-scripts(\s|$)/.test(frameInfo.sandbox) || /(^|\s)allow-same-origin(\s|$)/.test(frameInfo.sandbox)) {
    throw new Error('iframe sandbox must be allow-scripts without allow-same-origin: ' + frameInfo.sandbox);
  }
  await page.screenshot({ path: path.join(OUT, 'sideload-v2-6-open.png') });
  const frame = page.frames().find(f => f.url().includes('sidecraft'));
  if (!frame) throw new Error('no sidecraft frame, frames=' + page.frames().map(f => f.url()).join(','));
  const probe = await frame.evaluate(() => window.__probe ? window.__probe() : { error: 'no probe' });
  log('iframe probe', JSON.stringify(probe));
  const leaked = JSON.stringify(probe).includes('admin') && JSON.stringify(probe).includes('username');
  if (leaked) throw new Error('sideloaded app obtained the session: ' + JSON.stringify(probe));
  if (probe.parentReached) throw new Error('sandboxed app reached parent.document');
  if (!probe.parentError) throw new Error('expected parent.document to throw in the sandbox');

  // Save the opened file through the bridge (default document grant).
  await frame.evaluate(() => window.__save('hello.side', 'saved from sandbox', false));
  await page.waitForFunction(() => /Saved/.test(document.body.innerText), { timeout: 15000 }).catch(() => {});
  await sleep(800);
  const saved = fs.readFileSync(fileOnDisk('hello.side'), 'utf8');
  log('saved hello.side', JSON.stringify(saved));
  if (saved !== 'saved from sandbox') throw new Error('opened file was not saved, got ' + JSON.stringify(saved));

  // Create a new file through the shell Save As dialog.
  await frame.evaluate(() => window.__save('created.side', 'created via save as', true));
  await page.waitForFunction(() => [...document.querySelectorAll('.dialog h2, .dialog h1')].some(h => /Save as/i.test(h.textContent)), { timeout: 15000 });
  await sleep(300);
  await page.screenshot({ path: path.join(OUT, 'sideload-v2-7-save-as.png') });
  await page.evaluate(() => {
    const input = document.querySelector('.save-as-dialog input, .dialog label.stack input');
    if (input) { input.value = 'created.side'; input.dispatchEvent(new Event('input', { bubbles: true })); }
    const save = [...document.querySelectorAll('.dialog button')].find(b => b.textContent.trim() === 'Save');
    if (save) save.click();
  });
  await sleep(1500);
  if (!fs.existsSync(fileOnDisk('created.side'))) throw new Error('Save As did not create created.side');
  const created = fs.readFileSync(fileOnDisk('created.side'), 'utf8');
  log('created.side', JSON.stringify(created));
  if (created !== 'created via save as') throw new Error('created file content mismatch: ' + JSON.stringify(created));

  // Switch the same app to regular isolation and confirm it can use the session.
  await page.goto(BASE + '/admin/apps', { waitUntil: 'networkidle0' });
  await page.waitForSelector('li.app-row');
  await page.evaluate(() => {
    const row = [...document.querySelectorAll('li.app-row')].find(r => r.textContent.includes('SideCraft'));
    const b = row && [...row.querySelectorAll('button')].find(b => b.textContent.trim() === 'Isolation…');
    if (b) b.click();
  });
  await page.waitForFunction(() => [...document.querySelectorAll('.dialog input[type=radio]')].some(r => r.value === 'regular'), { timeout: 10000 });
  await page.evaluate(() => {
    const regular = [...document.querySelectorAll('.dialog input[type=radio]')].find(r => r.value === 'regular');
    if (regular) regular.click();
  });
  await page.waitForFunction(() => [...document.querySelectorAll('.dialog label')].some(l => /signed-in user/.test(l.textContent)), { timeout: 5000 });
  await page.evaluate(() => {
    const box = [...document.querySelectorAll('.dialog input[type=checkbox]')].find(b => /signed-in user/.test(b.parentElement.textContent));
    if (box && !box.checked) box.click();
    const save = [...document.querySelectorAll('.dialog button')].find(b => b.textContent.trim() === 'Save');
    if (save) save.click();
  });
  await page.waitForFunction(() => ![...document.querySelectorAll('.dialog h2, .dialog h1')].some(h => /Isolation for/.test(h.textContent)), { timeout: 15000 }).catch(() => {});
  await sleep(500);
  await page.goto(BASE + '/open/hello.side', { waitUntil: 'networkidle0' });
  await sleep(1500);
  const warning2 = await page.evaluate(() => document.querySelector('.dialog h2, .dialog p')?.textContent || '');
  log('regular first-open', warning2);
  if (/Unverified|trust list|administrator/i.test(warning2)) {
    await page.evaluate(() => [...document.querySelectorAll('.dialog button')].find(b => /anyway|Open/i.test(b.textContent))?.click());
  }
  await page.waitForSelector('section.editor iframe', { timeout: 20000 });
  await sleep(2000);
  const regularInfo = await page.$eval('section.editor iframe', f => ({ src: f.getAttribute('src') || '', sandbox: f.getAttribute('sandbox') }));
  log('regular iframe', regularInfo);
  if (regularInfo.sandbox) throw new Error('regular isolation must not set iframe sandbox, got ' + regularInfo.sandbox);
  const regularFrame = page.frames().find(f => f.url().includes('sidecraft'));
  if (!regularFrame) throw new Error('no sidecraft frame after switching to regular');
  const regularProbe = await regularFrame.evaluate(() => window.__probe ? window.__probe() : { error: 'no probe' });
  log('regular probe', JSON.stringify(regularProbe));
  const regularBody = JSON.stringify(regularProbe);
  if (!(regularBody.includes('admin') && regularBody.includes('username'))) {
    throw new Error('regular isolation should let the app use the signed-in session: ' + regularBody);
  }
  await page.screenshot({ path: path.join(OUT, 'sideload-v2-7b-regular.png') });

  // Update via the source, then disable, then read the audit log.
  await page.goto(BASE + '/admin/apps', { waitUntil: 'networkidle0' });
  await page.waitForSelector('li.app-row');
  await page.evaluate(() => {
    const row = [...document.querySelectorAll('li.app-row')].find(r => r.textContent.includes('SideCraft'));
    const b = row && [...row.querySelectorAll('button')].find(b => b.textContent.includes('Versions'));
    if (b) b.click();
  });
  await sleep(500);
  const checked = await page.evaluate(() => {
    const b = [...document.querySelectorAll('.dialog button')].find(b => b.textContent.includes('Check for versions'));
    if (b) { b.click(); return true; }
    return false;
  });
  log('check for versions', checked);
  await sleep(2000);
  const versionText = await page.evaluate(() => document.querySelector('.dialog')?.innerText || '');
  if (!/Checksum from the app's source/.test(versionText)) throw new Error('expected source checksum label, got: ' + versionText.slice(0, 400));
  if (/Checksum from the registry/.test(versionText) && !/Checksum from the app's source/.test(versionText)) {
    throw new Error('source version used the registry checksum label');
  }
  await page.screenshot({ path: path.join(OUT, 'sideload-v2-8-versions.png') });
  await page.evaluate(() => {
    const row = [...document.querySelectorAll('.version-row')].find(r => r.querySelector('strong')?.textContent.trim() === '0.2.0');
    const b = row && [...row.querySelectorAll('button')].find(b => b.textContent.trim() === 'Install');
    if (b) b.click();
  });
  await sleep(1000);
  await page.waitForFunction(() => document.body.innerText.includes('0.2.0 is installed') || document.body.innerText.includes('SideCraft 0.2.0') || document.body.innerText.includes('Failed.'), { timeout: 60000 }).catch(() => {});
  await sleep(400);
  await page.evaluate(() => [...document.querySelectorAll('.dialog button')].filter(b => /Close|Hide/.test(b.textContent)).forEach(b => b.click()));
  await sleep(300);
  await page.evaluate(() => {
    const row = [...document.querySelectorAll('li.app-row')].find(r => r.textContent.includes('SideCraft'));
    const b = row && [...row.querySelectorAll('button')].find(b => b.textContent.trim() === 'Disable');
    if (b) b.click();
  });
  await sleep(800);
  await page.evaluate(() => document.querySelector('.audit-log')?.scrollIntoView());
  await sleep(300);
  await page.screenshot({ path: path.join(OUT, 'sideload-v2-9-audit.png'), fullPage: true });
  const audit = await page.evaluate(() => document.querySelector('.audit-log')?.innerText || '');
  log('audit', audit.replace(/\n/g, ' | ').slice(0, 500));
  if (!/sidecraft/i.test(audit) && !/SideCraft/i.test(audit)) throw new Error('audit log missing sidecraft');

  await browser.close();
  server.close();
  if (errors.length) log('page errors', errors);
  log('ok');
})().catch(err => { console.error(err); process.exit(1); });
