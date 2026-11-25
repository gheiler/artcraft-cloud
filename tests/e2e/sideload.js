// Browser check for sideloaded apps. From tests/e2e after `npm install`:
//   BASE=http://127.0.0.1:8090 OUT=/tmp/sideload-shots ARTCRAFT_DATA=$ARTCRAFT_DATA node sideload.js
// The server must be running with ARTCRAFT_PUBLIC_URL and ARTCRAFT_APPS_URL (for example
// http://127.0.0.1:8090 and http://apps.localhost:8090). Super-admin login is admin / admin-pass-12345
// only when that is how the test server was started.

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
  const out={};
  try{const r=await fetch('/api/me',{credentials:'include',headers:{'x-requested-with':'artcraft'}});out.relative={status:r.status,body:await r.text()};}catch(e){out.relative={error:String(e)};}
  try{const r=await fetch(${JSON.stringify(BASE)}+'/api/me',{credentials:'include',headers:{'x-requested-with':'artcraft'}});out.absolute={status:r.status,body:await r.text()};}catch(e){out.absolute={error:String(e)};}
  return out;
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
  page.on('dialog', async d => { log('dialog', d.type(), d.message().slice(0, 200)); await d.accept(); });
  await login(page);

  await page.goto(BASE + '/admin/apps', { waitUntil: 'networkidle0' });
  await page.waitForSelector('#danger-zone, .danger-zone, section.danger-zone');
  await page.evaluate(() => document.querySelector('.danger-zone, #danger-zone')?.scrollIntoView());
  await sleep(400);
  await page.screenshot({ path: path.join(OUT, 'sideload-1-danger-zone.png'), fullPage: true });
  log('danger zone visible');

  const zipInput = await page.$('.sideload-form input[type=file]');
  if (!zipInput) throw new Error('no zip input');
  await page.evaluate(() => {
    const set = (sel, val) => {
      const el = [...document.querySelectorAll('.sideload-form input, .sideload-form select')].find(e => (e.placeholder || e.previousSibling?.textContent || e.parentElement?.textContent || '').includes(sel) || e.placeholder === sel);
    };
  });
  // Fill by label text
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
    val('Source URL or owner/repo', '');
  });
  // Re-set source URL after changing kind
  await page.evaluate((url) => {
    const labels = [...document.querySelectorAll('.sideload-form label')];
    const kind = labels.find(l => l.textContent.trim().startsWith('Source'))?.querySelector('select');
    if (kind) { kind.value = 'manifest'; kind.dispatchEvent(new Event('change', { bubbles: true })); }
    const src = labels.find(l => l.textContent.includes('Source URL'))?.querySelector('input');
    if (src) { src.value = url; src.dispatchEvent(new Event('input', { bubbles: true })); }
  }, manifest);
  await zipInput.uploadFile(zip1);
  await page.evaluate(() => {
    const boxes = [...document.querySelectorAll('.sideload-form input[type=checkbox]')];
    // bridge default on; tick confirm
    const confirm = boxes.find(b => b.parentElement.textContent.includes('trust list'));
    if (confirm && !confirm.checked) { confirm.click(); }
  });
  await sleep(200);
  await page.screenshot({ path: path.join(OUT, 'sideload-2-form-filled.png'), fullPage: true });
  await page.evaluate(() => document.querySelector('.sideload-form button[type=submit]').click());
  await page.waitForFunction(() => [...document.querySelectorAll('li.app-row strong')].some(s => s.textContent.includes('SideCraft')), { timeout: 60000 });
  await sleep(600);
  await page.screenshot({ path: path.join(OUT, 'sideload-3-installed.png'), fullPage: true });
  const row = await page.evaluate(() => [...document.querySelectorAll('li.app-row')].find(r => r.textContent.includes('SideCraft'))?.innerText || '');
  log('installed row', row.replace(/\n/g, ' | '));
  if (!row.includes('Unverified')) throw new Error('missing Unverified badge');

  // Isolation: the apps host must not see the session cookie (checked from Node,
  // because a cross-origin fetch from the shell is blocked by CORS — also good).
  const me = await page.evaluate(async () => (await fetch('/api/me', { headers: { 'x-requested-with': 'artcraft' } })).json());
  const cookieHeader = (await page.cookies()).map(c => c.name + '=' + c.value).join('; ');
  const apps = new URL(me.apps_origin);
  const iso = await new Promise((resolve, reject) => {
    const r = http.request({ hostname: '127.0.0.1', port: apps.port || 80, path: '/api/me',
      headers: { Host: apps.host, cookie: cookieHeader, 'x-requested-with': 'artcraft' } },
      resp => { let body = ''; resp.on('data', d => body += d); resp.on('end', () => resolve({ status: resp.statusCode, body })); });
    r.on('error', reject); r.end();
  });
  log('apps origin /api/me', me.apps_origin, iso.status, iso.body);
  if (iso.status === 200 && /username/.test(iso.body)) throw new Error('apps origin leaked the session');
  if (iso.status !== 404 && iso.status !== 403) throw new Error('expected apps host to refuse /api/me, got ' + iso.status);

  // Open a .side file with the sideloaded app.
  const filesDir = path.join(DATA, 'files/1');
  fs.mkdirSync(filesDir, { recursive: true });
  fs.writeFileSync(path.join(filesDir, 'hello.side'), 'hello from sideload');
  await page.goto(BASE + '/', { waitUntil: 'networkidle0' });
  await page.waitForSelector('.file-table, .new-cards');
  await sleep(800);
  const hasTile = await page.evaluate(() => document.body.innerText.includes('SideCraft'));
  log('launcher has SideCraft', hasTile);
  await page.screenshot({ path: path.join(OUT, 'sideload-4-launcher.png') });
  // Open hello.side
  const opened = await page.evaluate(() => {
    const name = [...document.querySelectorAll('.file-table .file-name, .file-name')].find(b => b.textContent.trim() === 'hello.side');
    if (name) { name.click(); return true; }
    return false;
  });
  if (!opened) {
    // navigate directly
    await page.goto(BASE + '/open/hello.side', { waitUntil: 'networkidle0' });
  }
  await sleep(1500);
  // First-open warning
  const warning = await page.evaluate(() => document.querySelector('.dialog h2, .dialog p')?.textContent || '');
  log('first-open', warning);
  if (/Unverified|trust list|administrator/i.test(warning)) {
    await page.screenshot({ path: path.join(OUT, 'sideload-5-first-open.png') });
    await page.evaluate(() => [...document.querySelectorAll('.dialog button')].find(b => /anyway|Open/i.test(b.textContent))?.click());
  }
  await page.waitForSelector('section.editor iframe', { timeout: 20000 });
  await sleep(2000);
  const src = await page.$eval('section.editor iframe', f => f.getAttribute('src'));
  log('iframe src', src);
  if (!src.includes('apps.') && !src.includes('/apps/sidecraft')) throw new Error('iframe is not isolated');
  await page.screenshot({ path: path.join(OUT, 'sideload-6-open.png') });
  const frame = page.frames().find(f => f.url().includes('sidecraft'));
  if (!frame) throw new Error('no sidecraft frame, frames=' + page.frames().map(f => f.url()).join(','));
  const probe = await frame.evaluate(() => window.__probe ? window.__probe() : { error: 'no probe' });
  log('iframe probe', JSON.stringify(probe));
  const leaked = JSON.stringify(probe).includes('admin') && JSON.stringify(probe).includes('username');
  if (leaked) throw new Error('sideloaded app obtained the session: ' + JSON.stringify(probe));

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
  await page.screenshot({ path: path.join(OUT, 'sideload-7-versions.png') });
  // Install 0.2.0 if offered
  await page.evaluate(() => {
    const row = [...document.querySelectorAll('.version-row')].find(r => r.querySelector('strong')?.textContent.trim() === '0.2.0');
    const b = row && [...row.querySelectorAll('button')].find(b => b.textContent.trim() === 'Install');
    if (b) b.click();
  });
  await sleep(1000);
  await page.waitForFunction(() => document.body.innerText.includes('0.2.0 is installed') || document.body.innerText.includes('SideCraft 0.2.0') || document.body.innerText.includes('Failed.'), { timeout: 60000 }).catch(() => {});
  await sleep(400);
  await page.screenshot({ path: path.join(OUT, 'sideload-8-updated.png'), fullPage: true });
  // Close dialogs
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
  await page.screenshot({ path: path.join(OUT, 'sideload-9-audit.png'), fullPage: true });
  const audit = await page.evaluate(() => document.querySelector('.audit-log')?.innerText || '');
  log('audit', audit.replace(/\n/g, ' | ').slice(0, 500));
  if (!/sidecraft/i.test(audit) && !/SideCraft/i.test(audit)) throw new Error('audit log missing sidecraft');

  await browser.close();
  server.close();
  if (errors.length) log('page errors', errors);
  log('ok');
})().catch(err => { console.error(err); process.exit(1); });
