const puppeteer = require('puppeteer-core');
const fs = require('fs');
const sleep = ms => new Promise(r => setTimeout(r, ms));
const BASE = process.env.BASE || 'http://127.0.0.1:8090';
const OUT = process.env.OUT || 'shots';
const FILES = (process.env.ARTCRAFT_DATA || 'data') + '/files/1/';
const only = process.argv[2] || 'all';
const errors = [];
async function login(page) {
  await page.goto(BASE + '/', { waitUntil: 'networkidle0' });
  await page.type('input[autocomplete=username]', 'admin'); await page.type('input[type=password]', process.env.ARTCRAFT_ADMIN_PASSWORD || 'admin-pass-12345');
  await page.click('button[type=submit]'); await page.waitForSelector('.file-table .row');
}
const clickFile = (page, name) => page.evaluate(n => [...document.querySelectorAll('.file-table .file-name')].find(b => b.textContent.trim() === n).click(), name);
const frame = page => page.frames().find(f => f.url().includes('/apps/'));
async function openDoc(page, name) { await clickFile(page, name); await page.waitForSelector('section.editor iframe'); await sleep(9000); }
async function back(page) { await page.click('.doc-bar button[aria-label="Back to your files"]'); await sleep(1200); }
(async () => {
  const browser = await puppeteer.launch({ executablePath: process.env.CHROME || '/usr/bin/google-chrome', headless: 'new', args: ['--no-sandbox', '--use-angle=swiftshader', '--enable-unsafe-swiftshader', '--window-size=1440,900'] });
  const page = await browser.newPage(); await page.setViewport({ width: 1440, height: 900 });
  page.on('pageerror', e => errors.push('pageerror ' + e.message.slice(0, 300)));
  page.on('console', m => { if (m.type() === 'error' && !m.text().includes('401')) errors.push('console ' + m.text().slice(0, 300)); });
  page.on('dialog', async d => { errors.push('NATIVE ' + d.type() + ' ' + d.message()); await d.accept(); });
  await login(page);

  if (only === 'all' || only === 'open') {
    await openDoc(page, 'hello.docx');
    await frame(page).evaluate(() => window.wordcraft.execute('file.open', {}));
    await page.waitForSelector('.dialog', { timeout: 5000 }).catch(() => errors.push('no picker dialog'));
    await sleep(800); await page.screenshot({ path: OUT + '/in-app-open-picker.png' });
    await page.evaluate(() => [...document.querySelectorAll('.dialog .file-name')].find(b => b.textContent.includes('hello (copy).docx')).click());
    await sleep(9000);
    console.log('A open: url', page.url(), 'name', await page.$eval('.doc-name strong', e => e.textContent));
    // Insert picture through the app's picker
    await frame(page).evaluate(() => window.wordcraft.execute('insert.picture', {}));
    await page.waitForSelector('.dialog', { timeout: 5000 }).catch(() => errors.push('no picture picker'));
    const listed = await page.$$eval('.dialog .file-name .label', e => e.map(x => x.textContent));
    console.log('B picture picker lists', JSON.stringify(listed));
    await page.evaluate(() => [...document.querySelectorAll('.dialog .file-name')].find(b => b.textContent.includes('sample.jpg')).click());
    await sleep(2500);
    console.log('B dialog closed', !(await page.$('.dialog')), 'dirty', !!(await page.$('.doc-bar .dot')));
    await page.screenshot({ path: OUT + '/after-picture.png' });
    // Discard confirm
    await back(page);
    const discard = await page.$eval('.dialog h2', e => e.textContent).catch(() => null);
    console.log('G discard dialog', discard);
    await page.screenshot({ path: OUT + '/discard-confirm.png' });
    if (discard) { await page.evaluate(() => [...document.querySelectorAll('.dialog button')].find(b => b.textContent.includes('Discard')).click()); await sleep(1500); }
    console.log('G after discard url', page.url());
  }
  if (only === 'all' || only === 'autosave') {
    await page.goto(BASE + '/', { waitUntil: 'networkidle0' }); await page.waitForSelector('.file-table .row');
    await openDoc(page, 'Book1.xlsx');
    const before = fs.statSync(FILES + 'Book1.xlsx').mtimeMs;
    const fr = frame(page); const c = await fr.$('canvas'); const b = await c.boundingBox();
    await page.mouse.click(b.x + b.width * 0.35, b.y + b.height * 0.45); await page.keyboard.type('autosaved ' + Date.now() % 1000); await page.keyboard.press('Enter');
    await sleep(14000);
    const after = fs.statSync(FILES + 'Book1.xlsx').mtimeMs;
    console.log('C autosave wrote file', after !== before, 'status', await page.$eval('.doc-bar .status', e => e.textContent), 'dirty', !!(await page.$('.doc-bar .dot')));
    await page.screenshot({ path: OUT + '/gridcraft-autosave.png' });
    await back(page);
  }
  if (only === 'all' || only === 'settings') {
    await page.goto(BASE + '/', { waitUntil: 'networkidle0' }); await page.waitForSelector('.file-table .row');
    await openDoc(page, 'hello.docx');
    await frame(page).evaluate(() => localStorage.setItem('wordcraft/probe.json', JSON.stringify({ author: 'Gabriel Heiler', initials: 'GH', recent: ['/Hola.docx'] })));
    await sleep(2500);
    const saved = await page.evaluate(async () => (await fetch('/api/settings/wordcraft', { headers: { 'x-requested-with': 'artcraft' } })).text());
    console.log('D saved keys', Object.keys(JSON.parse(saved)));
    await back(page);
    await page.evaluate(() => { Object.keys(localStorage).filter(k => k.startsWith('wordcraft')).forEach(k => localStorage.removeItem(k)); });
    await openDoc(page, 'hello.docx');
    const restored = await page.evaluate(() => localStorage.getItem('wordcraft/probe.json'));
    const src = await page.$eval('section.editor iframe', f => f.getAttribute('src'));
    console.log('D restored', restored, 'src', src);
    await back(page);
  }
  if (only === 'all' || only === 'zip') {
    await page.goto(BASE + '/', { waitUntil: 'networkidle0' }); await page.waitForSelector('.file-table .row');
    await clickFile(page, 'My Backup.zip'); await page.waitForSelector('section.editor iframe', { timeout: 8000 }).catch(() => errors.push('zip did not open'));
    await sleep(8000);
    console.log('E zip app', await page.$eval('section.editor iframe', f => f.getAttribute('src')).catch(() => 'none'), 'banner', await page.$eval('.note.banner', e => e.textContent).catch(() => 'none'));
    await page.screenshot({ path: OUT + '/lightcraft-backup.png' });
    await back(page);
    // Open with menu
    await page.evaluate(() => { const row = [...document.querySelectorAll('.file-table .row')].find(r => r.textContent.includes('-test.txt')); row.querySelector('button[aria-label="Open with"]').click(); });
    await page.waitForSelector('.dialog'); await sleep(400);
    await page.screenshot({ path: OUT + '/open-with.png' });
    await page.keyboard.press('Escape'); await sleep(300);
    console.log('E escape closes', !(await page.$('.dialog')));
  }
  if (only === 'all' || only === 'new') {
    await page.goto(BASE + '/', { waitUntil: 'networkidle0' }); await page.waitForSelector('.new-card');
    await page.evaluate(() => [...document.querySelectorAll('.new-card')].find(b => b.textContent.includes('WordCraft')).click());
    await page.waitForSelector('section.editor iframe'); await sleep(9000);
    console.log('F new url', page.url());
    await frame(page).evaluate(() => window.wordcraft.execute('file.new', {})).catch(e => console.log('file.new', e.message)); await sleep(2500);
    const fr = frame(page); const c = await fr.$('canvas'); const b = await c.boundingBox();
    await page.mouse.click(b.x + b.width * 0.5, b.y + b.height * 0.4); await page.keyboard.type('Hello from a new doc');
    await sleep(500); await frame(page).evaluate(() => window.wordcraft.execute('file.save', {})); await sleep(2500); await page.screenshot({ path: OUT + '/new-save.png' });
    const form = await page.$('form.save-as');
    console.log('F save-as shown', !!form);
    if (form) { await page.evaluate(() => { const i = document.querySelector('form.save-as input'); i.value = ''; }); await page.type('form.save-as input', 'Note ' + Date.now() % 10000 + '.docx'); await page.click('form.save-as button[type=submit]'); await sleep(2500); }
    console.log('F url after save', page.url());
  }
  if (only === 'pdf') {
    await openDoc(page, 'sample.pdf');
    const fr = frame(page); const c = await fr.$('canvas'); const b = await c.boundingBox();
    await page.mouse.click(b.x + b.width * 0.5, b.y + b.height * 0.5);
    await page.keyboard.down('Control'); await page.keyboard.press('o'); await page.keyboard.up('Control'); await sleep(2000);
    console.log('K pdf ctrl+o dialog', await page.$eval('.dialog h2', e => e.textContent).catch(() => null));
    await page.screenshot({ path: OUT + '/pdf-open.png' });
  }
  if (only === 'cmds') {
    for (const [file, app] of [['Presentation1.deckcraft','deckcraft'],['sample.pdf','pdfcraft'],['sample.dxf','cadcraft']]) {
      await page.goto(BASE + '/', { waitUntil: 'networkidle0' }); await page.waitForSelector('.file-table .row');
      await openDoc(page, file);
      const r = await frame(page).evaluate(async a => { const o = window[a] || {}; let c0 = null; try { c0 = await o.commands(); } catch (e) { c0 = 'e ' + e.message; } const s0 = JSON.stringify(c0); const opens = (s0.match(/"[a-zA-Z_.]*(open|Open|import|recent)[a-zA-Z_.]*"/g) || []).slice(0, 30); let cmds = null; try { cmds = (o.commands ? o.commands() : o.listCommands ? o.listCommands() : null); } catch (e) { cmds = 'err ' + e.message; } return { keys: Object.keys(o), globals: Object.keys(window).filter(k => k.toLowerCase().includes(a.slice(0,4))), cmds: JSON.stringify(cmds)?.slice(0, 300), opens, len: s0.length, head: s0.slice(0, 400) }; }, app);
      console.log('J', app, JSON.stringify(r));
    }
  }
  if (only === 'others') {
    for (const [file, app, cmd] of [['Presentation1.deckcraft','deckcraft','app.openDialog'],['sample.dxf','cadcraft','ui.open'],['sample.dxf','cadcraft','open']]) {
      await page.goto(BASE + '/', { waitUntil: 'networkidle0' }); await page.waitForSelector('.file-table .row');
      await openDoc(page, file);
      const r = await frame(page).evaluate(a => { try { const x = window[a[0]].execute(a[1], {}); return 'ok ' + JSON.stringify(x ?? null).slice(0,80); } catch (e) { return 'err ' + e.message; } }, [app, cmd]).catch(e => 'eval ' + e.message);
      await sleep(1500);
      const dlg = await page.$eval('.dialog h2', e => e.textContent).catch(() => null);
      console.log('I', app, cmd, r, 'dialog', dlg);
      if (dlg) await page.keyboard.press('Escape');
      await sleep(500);
    }
  }
  if (only === 'all' || only === 'dialogs') {
    await page.goto(BASE + '/', { waitUntil: 'networkidle0' }); await page.waitForSelector('.file-table .row');
    const act = (name, label) => page.evaluate((n, l) => { const row = [...document.querySelectorAll('.file-table .row')].find(r => r.textContent.includes(n)); row.querySelector(`button[aria-label="${l}"]`).click(); }, name, label);
    await act('-test.txt', 'Rename'); await page.waitForSelector('.dialog'); await sleep(300); await page.screenshot({ path: OUT + '/rename-dialog.png' }); await page.keyboard.press('Escape');
    await act('-test.txt', 'Move'); await page.waitForSelector('.dialog'); await sleep(600); await page.screenshot({ path: OUT + '/move-dialog.png' }); await page.keyboard.press('Escape');
    await act('-test.txt', 'Move to trash'); await page.waitForSelector('.dialog'); await sleep(300); await page.screenshot({ path: OUT + '/delete-confirm.png' }); await page.keyboard.press('Escape');
    await page.click('.tools button.button'); await page.waitForSelector('.dialog'); await page.type('.dialog input', 'Projects'); await page.keyboard.press('Enter'); await sleep(1200);
    console.log('H folder created', fs.existsSync(FILES + 'Projects'));
  }
  console.log('errors', JSON.stringify(errors, null, 1));
  await browser.close();
})().catch(e => { console.error(e); console.log('errors', JSON.stringify(errors)); process.exit(1); });
