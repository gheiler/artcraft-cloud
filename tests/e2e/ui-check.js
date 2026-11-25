const puppeteer = require('puppeteer-core');
const sleep = ms => new Promise(r => setTimeout(r, ms));
const BASE = process.env.BASE || 'http://127.0.0.1:8090';
const OUT = process.env.OUT || 'shots';
(async () => {
  const browser = await puppeteer.launch({ executablePath: process.env.CHROME || '/usr/bin/google-chrome', headless: 'new', args: ['--no-sandbox', '--use-angle=swiftshader', '--enable-unsafe-swiftshader', '--window-size=1440,900'] });
  const page = await browser.newPage(); await page.setViewport({ width: 1440, height: 900 });
  const errors = [];
  page.on('console', m => { if (m.type() === 'error') errors.push(m.text()); });
  page.on('pageerror', e => errors.push('pageerror ' + e.message));
  page.on('dialog', async d => { errors.push('NATIVE DIALOG ' + d.type() + ': ' + d.message()); await d.dismiss(); });
  await page.goto(BASE + '/', { waitUntil: 'networkidle0' });
  await page.waitForSelector('input[autocomplete=username]');
  await page.type('input[autocomplete=username]', 'admin'); await page.type('input[type=password]', process.env.ARTCRAFT_ADMIN_PASSWORD || 'admin-pass-12345');
  await page.click('button[type=submit]');
  await page.waitForSelector('.file-table .row'); await sleep(800);
  await page.screenshot({ path: OUT + '/drive.png' });
  // People on first open (the old panic)
  await page.evaluate(() => [...document.querySelectorAll('.nav')].find(b => b.textContent.includes('People')).click());
  await page.waitForSelector('.people .file-table .row', { timeout: 8000 }).catch(() => errors.push('people list did not render'));
  console.log('people rows', await page.$$eval('.people .file-table tbody .row', r => r.length));
  await sleep(500); await page.screenshot({ path: OUT + '/people.png' });
  await page.evaluate(() => [...document.querySelectorAll('.nav')].find(b => b.textContent.includes('My files')).click());
  await page.waitForSelector('.file-table .row');
  // Share dialog
  await page.evaluate(() => { const row = [...document.querySelectorAll('.file-table .row')].find(r => r.textContent.includes('hello.docx')); row.querySelector('button[aria-label=Share]').click(); });
  await page.waitForSelector('.dialog'); await sleep(700);
  await page.focus('.dialog input'); await page.keyboard.type('a'); await sleep(500);
  await page.screenshot({ path: OUT + '/share-dialog.png' });
  await page.keyboard.press('Escape'); await sleep(300);
  // New-file cards close-up
  const cards = await page.$('.new-cards'); await cards.screenshot({ path: OUT + '/new-file-cards.png' });
  // Open a doc from the drive
  await page.evaluate(() => [...document.querySelectorAll('.file-name')].find(b => b.textContent.includes('hello.docx')).click());
  await page.waitForSelector('section.editor iframe'); await sleep(9000);
  console.log('url after open', page.url());
  await page.screenshot({ path: OUT + '/doc-from-drive.png' });
  // Direct link in a fresh tab
  const page2 = await browser.newPage(); await page2.setViewport({ width: 1440, height: 900 });
  page2.on('pageerror', e => errors.push('page2 ' + e.message));
  await page2.goto(BASE + '/open/Book1.xlsx', { waitUntil: 'networkidle0' });
  await page2.waitForSelector('section.editor iframe'); await sleep(9000);
  console.log('pill', !!(await page2.$('.home-pill')), 'bar', !!(await page2.$('.doc-bar')));
  await page2.screenshot({ path: OUT + '/fullscreen-doc.png' });
  console.log('errors', JSON.stringify(errors, null, 1));
  await browser.close();
})().catch(e => { console.error(e); process.exit(1); });
