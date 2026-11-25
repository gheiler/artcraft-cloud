// Mail end to end against a real Stalwart (docs/mail.md): the super admin connects the server and
// sees a domain's DNS records, People creates a person with a mailbox (whose password is theirs),
// a person changes their own password (and IMAP follows), and MailCraft opens that person's inbox
// through the JMAP proxy.
//
// Needs: artcraftCloud at BASE with MailCraft installed, Stalwart reachable from it, the mail
// config set (or MAIL_URL/MAIL_ADMIN/MAIL_PASS to make an API key from that login), jonas with a
// mailbox holding mail. IMAP_URL (e.g. imaps://127.0.0.1:9993) adds the IMAP checks (curl).
// The test person it creates gets their mailbox deleted and is disabled at the end.
const puppeteer = require('puppeteer-core');
const { spawnSync } = require('child_process');
const IMAP_URL = process.env.IMAP_URL || '';
// true when address/password opens INBOX over IMAP.
const imap = (address, password) => spawnSync('curl', ['-sk', '--max-time', '10', '--url', IMAP_URL + '/INBOX', '--user', `${address}:${password}`, '-X', 'EXAMINE INBOX']).status === 0;
const sleep = ms => new Promise(r => setTimeout(r, ms));
const BASE = process.env.BASE || 'http://127.0.0.1:8090';
const OUT = process.env.OUT || '/workspace/artcraft-cloud-shots';
const ADMIN = process.env.E2E_ADMIN || 'admin', ADMIN_PASS = process.env.E2E_ADMIN_PASS || 'admin-pass-12345';
const USER = process.env.E2E_USER || 'jonas', PASS = process.env.E2E_PASS || 'pass-jonas-12345';
const DOMAIN = process.env.MAIL_DOMAIN || 'example.test';
const fails = [];
const check = (ok, what) => { console.log((ok ? 'ok   ' : 'FAIL ') + what); if (!ok) fails.push(what); };
const api = (page, method, url, body) => page.evaluate(async (method, url, body) => {
  const r = await fetch(url, { method, headers: { 'x-requested-with': 'artcraft', 'content-type': 'application/json' }, body: body === undefined ? undefined : JSON.stringify(body) });
  const text = await r.text(); let json; try { json = JSON.parse(text); } catch { json = text; } return { status: r.status, json };
}, method, url, body);
const quiet = async page => { await page.evaluate(() => document.querySelectorAll('.toast .icon-button').forEach(b => b.click())); await sleep(300); };
const toast = page => page.evaluate(() => document.querySelector('.toast')?.textContent?.trim() || '');
async function signIn(browser, user, pass) {
  const context = await browser.createBrowserContext();
  const page = await context.newPage(); await page.setViewport({ width: 1440, height: 900 });
  page.on('pageerror', e => fails.push(`pageerror (${user}) ` + e.message.slice(0, 200)));
  await page.goto(BASE + '/', { waitUntil: 'networkidle0' });
  await page.type('input[autocomplete=username]', user); await page.type('input[type=password]', pass);
  await page.click('button[type=submit]'); await page.waitForSelector('.sidebar nav');
  await api(page, 'PUT', '/api/preferences', { theme: 'light', language: 'en' });
  return page;
}
(async () => {
  const browser = await puppeteer.launch({ executablePath: '/usr/bin/google-chrome', headless: 'new', args: ['--no-sandbox', '--use-angle=swiftshader', '--enable-unsafe-swiftshader', '--window-size=1440,900', '--lang=en-US'] });

  // 1. The super admin: Mail page.
  const admin = await signIn(browser, ADMIN, ADMIN_PASS);
  if (process.env.MAIL_URL) {
    const r = await api(admin, 'PUT', '/api/mail/config', { base_url: process.env.MAIL_URL, auth: 'basic', username: process.env.MAIL_ADMIN, secret: process.env.MAIL_PASS, create_api_key: true });
    check(r.status === 200 && r.json.auth === 'bearer' && !r.json.username, 'an API key was made from the admin login, and only the key kept');
  }
  const config = await api(admin, 'GET', '/api/mail/config');
  check(config.status === 200 && config.json.has_secret && !JSON.stringify(config.json).includes(process.env.MAIL_PASS || '\u0000'), 'config comes back without the secret');
  await admin.goto(BASE + '/mail', { waitUntil: 'networkidle0' }); await admin.waitForSelector('.mail-page');
  const options = await admin.$$eval('.mail-page select option', e => e.map(x => x.textContent.trim()));
  check(options[0] === 'API key (recommended)' && options[1] === 'Admin account (fallback)', 'API key first and recommended: ' + options.join(' / '));
  check((await admin.$$eval('.sidebar nav .nav-label', e => e.map(x => x.textContent.trim()))).includes('Mail'), 'Mail is in the sidebar for the super admin');
  const testButton = await admin.waitForSelector('xpath/.//button[contains(., "Test connection")]');
  await testButton.click(); await admin.waitForSelector('.test-result', { timeout: 10000 }); await sleep(300);
  const result = await admin.$eval('.test-result', e => e.textContent);
  check(result.includes('Connected.') && result.includes('manage'), 'test connection: ' + result.trim());
  await admin.waitForSelector('details.domain');
  const names = await admin.$$eval('details.domain summary strong', e => e.map(x => x.textContent));
  check(names.includes(DOMAIN), 'domain listed: ' + names.join(', '));
  await admin.evaluate(d => { const el = [...document.querySelectorAll('details.domain')].find(x => x.querySelector('strong').textContent === d); el.open = true; }, DOMAIN);
  await sleep(300);
  const purposes = await admin.$$eval('details.domain[open] tbody tr td:first-child', e => e.map(x => x.textContent));
  check(['MX', 'SPF', 'DKIM', 'DMARC'].every(p => purposes.includes(p)), 'MX, SPF, DKIM and DMARC records shown');
  await quiet(admin);
  await admin.setViewport({ width: 1440, height: 1250 });
  await admin.screenshot({ path: OUT + '/mail-settings.png' });
  await admin.setViewport({ width: 1440, height: 900 });

  // 2. People: a new person with a mailbox, then the Email dialog.
  const who = 'e2email' + Date.now().toString(36).slice(-4);
  const WHO_PASS = 'pass-e2e-12345';
  await admin.goto(BASE + '/people', { waitUntil: 'networkidle0' }); await admin.waitForSelector('.people form');
  await admin.type('.people form input[placeholder=Username]', who);
  await admin.type('.people form input[placeholder="Full name (optional)"]', 'Lena Vogel');
  await admin.type('.people form input[type=email]', `${who}@${DOMAIN}`);
  await admin.type('.people form input[type=password]', WHO_PASS);
  await admin.waitForSelector('.mailbox-check', { timeout: 5000 });
  check(await admin.$eval('.mailbox-check input', e => e.checked), '"Create the mailbox" offered and on for a served domain');
  await admin.click('.people form button[type=submit]'); await sleep(1500);
  check((await toast(admin)).includes('with the mailbox'), 'created with a mailbox: ' + await toast(admin));
  const users = (await api(admin, 'GET', '/api/users')).json;
  const lena = users.find(u => u.username === who);
  check(lena && lena.mailbox && lena.email === `${who}@${DOMAIN}`, 'the person is linked to the mailbox');
  check(lena && lena.mail_sync === 'synced', 'the mailbox has their password from the start');
  if (IMAP_URL) check(imap(`${who}@${DOMAIN}`, WHO_PASS), 'IMAP sign-in with the new person\'s artcraftCloud password');
  // Disabling the person suspends the mailbox on the mail server; enabling restores it.
  const rowButton = (name, label) => admin.evaluate((name, label) => { const row = [...document.querySelectorAll('tr.row')].find(r => r.textContent.includes(name)); [...row.querySelectorAll('button')].find(b => b.textContent.trim() === label).click(); }, name, label);
  await quiet(admin); await rowButton(who, 'Disable'); await sleep(1500);
  check((await toast(admin)).includes('mailbox suspended'), 'disabled, mailbox suspended: ' + await toast(admin));
  const off = (await api(admin, 'GET', '/api/users')).json.find(u => u.username === who);
  check(off.disabled && off.mail_suspended && !off.mail_access_error, 'People lists the mailbox as suspended');
  check(await admin.evaluate(name => [...document.querySelectorAll('tr.row')].find(r => r.textContent.includes(name)).textContent.includes('mailbox suspended'), who), 'the row shows "mailbox suspended"');
  await admin.screenshot({ path: OUT + '/people-mail-suspended.png' });
  if (IMAP_URL) check(!imap(`${who}@${DOMAIN}`, WHO_PASS), 'IMAP refuses the disabled person');
  await quiet(admin);
  await rowButton(who, 'Enable'); await sleep(1500);
  check((await toast(admin)).includes('mailbox restored'), 'enabled, mailbox restored: ' + await toast(admin));
  const on = (await api(admin, 'GET', '/api/users')).json.find(u => u.username === who);
  check(!on.disabled && !on.mail_suspended, 'People lists the mailbox as active again');
  if (IMAP_URL) check(imap(`${who}@${DOMAIN}`, WHO_PASS), 'IMAP lets them in again');
  await quiet(admin);
  await admin.evaluate(name => { const row = [...document.querySelectorAll('tr.row')].find(r => r.textContent.includes(name)); [...row.querySelectorAll('button')].find(b => b.textContent.trim() === 'Email').click(); }, who);
  await admin.waitForSelector('.dialog .mailbox-box');
  const deleteBtn = await admin.waitForSelector('xpath/.//button[contains(., "Delete mailbox…")]');
  await deleteBtn.click(); await admin.waitForSelector('.danger-zone input');
  const finalBtn = () => admin.$eval('.danger-zone .button.danger', b => b.disabled);
  check(await finalBtn(), 'deleting is disabled until the address is typed');
  await admin.type('.danger-zone input', `${who}@${DOMAIN}`);
  check(!(await finalBtn()), 'typing the address enables deleting');
  await admin.screenshot({ path: OUT + '/user-email.png' });
  const finalDelete = await admin.waitForSelector('.danger-zone .button.danger'); await finalDelete.click(); await sleep(1500);
  check((await toast(admin)).includes('deleted from the mail server'), 'deleted with the typed address: ' + await toast(admin));
  if (IMAP_URL) check(!imap(`${who}@${DOMAIN}`, WHO_PASS), 'and the account is gone from IMAP');
  await admin.keyboard.press('Escape');
  // The test person cannot be deleted (artcraftCloud has no user deletion yet): disable them.
  check((await api(admin, 'POST', `/api/users/${lena.id}/disable`)).status === 200, 'the test person is disabled');

  // 3. A person changes their own password.
  const page = await signIn(browser, USER, PASS);
  const other_session = await signIn(browser, USER, PASS);
  // A mailbox made before its password was known gets it at sign-in (in the background).
  for (let i = 0; i < 20 && (await api(page, 'GET', '/api/mail/me')).json.password_sync !== 'synced'; i++) await sleep(500);
  const mine = (await api(page, 'GET', '/api/mail/me')).json;
  check(mine.password_sync === 'synced', `${USER}'s mailbox has their password: ${mine.password_sync}`);
  if (IMAP_URL) check(imap(mine.mailbox, PASS), 'IMAP sign-in with the artcraftCloud password');
  await page.goto(BASE + '/settings', { waitUntil: 'networkidle0' }); await page.waitForSelector('.account-card');
  await page.waitForSelector('.mail-client', { timeout: 10000 });
  const info = await page.$eval('.mail-client', e => e.textContent);
  check(info.includes(mine.mailbox) && info.includes('993') && info.includes('465'), 'Settings › Account shows the IMAP/SMTP settings');
  const card = await page.$('.account-card');
  await card.screenshot({ path: OUT + '/mail-client-info.png' });
  const fill = async (cur, next, again) => {
    for (const [sel, v] of [['input[autocomplete=current-password]', cur], ['.password-fields input[autocomplete=new-password]', next]]) { await page.click(sel, { clickCount: 3 }); await page.keyboard.press('Backspace'); await page.type(sel, v); }
    const fields = await page.$$('.password-fields input[autocomplete=new-password]');
    await fields[1].click({ clickCount: 3 }); await page.keyboard.press('Backspace'); await fields[1].type(again);
  };
  await fill('wrong-password-1', 'pass-jonas-67890', 'pass-jonas-6789');
  check(await page.$('.form-error') !== null, 'mismatched confirmation is flagged');
  check(await page.$eval('.password-form button[type=submit]', b => b.disabled), 'submit stays disabled while they differ');
  await fill('wrong-password-1', 'pass-jonas-67890', 'pass-jonas-67890');
  await page.click('.password-form button[type=submit]'); await sleep(1000);
  check((await page.$eval('.form-error', e => e.textContent).catch(() => '')).includes('not right'), 'a wrong current password is refused');
  await fill(PASS, 'pass-jonas-67890', 'pass-jonas-67890');
  await sleep(300); await page.screenshot({ path: OUT + '/change-password.png' });
  await page.click('.password-form button[type=submit]'); await sleep(1200);
  check((await toast(page)).includes('and your mailbox'), 'password changed for both: ' + await toast(page));
  check((await api(page, 'GET', '/api/me')).status === 200, 'this browser stays signed in');
  check((await api(other_session, 'GET', '/api/me')).status === 401, 'the other session was signed out');
  if (IMAP_URL) {
    check(imap(mine.mailbox, 'pass-jonas-67890'), 'IMAP works with the new password');
    check(!imap(mine.mailbox, PASS), 'and no longer with the old one');
  }
  // MailCraft still opens the inbox (it has its own app password), checked in 4 before changing back.
  const proxied = await page.evaluate(async () => {
    const t = await (await fetch('/api/mail/token', { method: 'POST', headers: { 'x-requested-with': 'artcraft' } })).json();
    return (await fetch('/jmap/session', { headers: { authorization: 'Bearer ' + t.token } })).status;
  });
  check(proxied === 200, 'the JMAP proxy still signs in after the change');

  // 4. MailCraft opens the person's inbox.
  const mail = (await api(page, 'GET', '/api/mail/me')).json;
  check(mail.mailbox, `${USER} has a mailbox: ${mail.mailbox}`);
  const jmap = [];
  page.on('response', r => { if (r.url().includes('/jmap')) jmap.push(`${r.request().method()} ${new URL(r.url()).pathname} ${r.status()}`); });
  await page.evaluate(() => Object.keys(localStorage).filter(k => k.startsWith('mailcraft')).forEach(k => localStorage.removeItem(k)));
  await page.goto(BASE + '/settings', { waitUntil: 'networkidle0' });
  const open = await page.waitForSelector('xpath/.//button[contains(., "Open MailCraft")]'); await open.click();
  await page.waitForSelector('section.editor iframe');
  const frame = await (async () => { for (let i = 0; i < 60; i++) { const f = page.frames().find(f => f.url().includes('/apps/mailcraft')); if (f) return f; await sleep(500); } })();
  check(!!frame && !frame.url().includes('host=parent'), 'MailCraft runs without the host bridge: ' + frame?.url());
  const hooked = await frame.evaluate(() => window.mailcraftJmap?.accounts?.[0]?.email).catch(() => null);
  check(hooked === mail.mailbox, 'the page named the mailbox to MailCraft: ' + hooked);
  for (let i = 0; i < 40 && !jmap.some(x => x.startsWith('POST /jmap/ 200')); i++) await sleep(1000);
  await sleep(6000);
  check(jmap.some(x => x.startsWith('GET /jmap/session 200')), 'MailCraft fetched the session through the proxy');
  check(jmap.some(x => x.startsWith('POST /jmap/ 200')), 'and read mail through it: ' + [...new Set(jmap)].join(', '));
  const stored = await frame.evaluate(() => localStorage.getItem('mailcraft.mailbox.v1') || '');
  check(stored.includes(mail.mailbox), 'MailCraft keeps the account in its own mailbox');
  const settings = (await api(page, 'GET', '/api/settings/mailcraft')).json;
  check(!('mailcraft.mailbox.v1' in settings), 'the mail itself is not uploaded to the drive settings');
  await page.screenshot({ path: OUT + '/mailcraft-inbox.png' });
  const back = await api(page, 'POST', '/api/me/password', { current: 'pass-jonas-67890', next: PASS });
  check(back.status === 200 && back.json.mail === 'synced', 'and back again: ' + JSON.stringify(back.json));
  if (IMAP_URL) check(imap(mine.mailbox, PASS), 'IMAP follows back');

  console.log(fails.length ? `\n${fails.length} failed` : '\nall passed');
  await browser.close();
  process.exit(fails.length ? 1 : 0);
})().catch(e => { console.error(e); process.exit(2); });
