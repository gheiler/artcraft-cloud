// Settings end to end: theme, shell language, and an app launched in the user's language.
const puppeteer = require('puppeteer-core');
const sleep = ms => new Promise(r => setTimeout(r, ms));
const BASE = process.env.BASE || 'http://127.0.0.1:8090';
const OUT = process.env.OUT || 'shots';
const USER = process.env.E2E_USER || 'jonas', PASS = process.env.E2E_PASS || 'pass-jonas-12345';
const fails = [];
const check = (ok, what) => { console.log((ok ? 'ok   ' : 'FAIL ') + what); if (!ok) fails.push(what); };
const api = (page, method, url, body) => page.evaluate(async (method, url, body) => {
  const r = await fetch(url, { method, headers: { 'x-requested-with': 'artcraft', 'content-type': 'application/json' }, body: body === undefined ? undefined : JSON.stringify(body) });
  const text = await r.text(); try { return JSON.parse(text); } catch { return text; }
}, method, url, body);
const quiet = async page => { await page.evaluate(() => document.querySelectorAll('.toast .icon-button').forEach(b => b.click())); await sleep(400); };
const navLabels = page => page.$$eval('.sidebar nav .nav-label', e => e.map(x => x.textContent.trim()));
(async () => {
  const browser = await puppeteer.launch({ executablePath: process.env.CHROME || '/usr/bin/google-chrome', headless: 'new', args: ['--no-sandbox', '--use-angle=swiftshader', '--enable-unsafe-swiftshader', '--window-size=1440,900', '--lang=en-US'] });
  const page = await browser.newPage(); await page.setViewport({ width: 1440, height: 900 });
  page.on('pageerror', e => fails.push('pageerror ' + e.message.slice(0, 200)));
  await page.goto(BASE + '/', { waitUntil: 'networkidle0' });
  await page.type('input[autocomplete=username]', USER); await page.type('input[type=password]', PASS);
  await page.click('button[type=submit]'); await page.waitForSelector('.sidebar nav');
  // Start from defaults with nothing stored for WordCraft.
  await api(page, 'PUT', '/api/preferences', {});
  await api(page, 'PUT', '/api/settings/wordcraft', {});
  await page.goto(BASE + '/settings', { waitUntil: 'networkidle0' }); await page.waitForSelector('.settings-page');
  await page.click('.segment:has(input[value=light])'); await sleep(800);
  check(await page.evaluate(() => document.documentElement.dataset.theme) === 'light', 'light theme set on <html>');
  await page.setViewport({ width: 1440, height: 1180 }); await quiet(page);
  await page.screenshot({ path: OUT + '/settings.png' });

  await page.click('.segment:has(input[value=dark])'); await sleep(800);
  const dark = await page.evaluate(() => ({ theme: document.documentElement.dataset.theme, bg: getComputedStyle(document.documentElement).backgroundColor, text: getComputedStyle(document.body).color }));
  check(dark.theme === 'dark' && dark.bg === 'rgb(28, 28, 28)' && dark.text === 'rgb(255, 255, 255)', 'dark mode renders ' + JSON.stringify(dark));
  await quiet(page); await page.screenshot({ path: OUT + '/settings-dark.png' });
  await page.setViewport({ width: 1440, height: 900 });

  const name = 'Jonas Weber-Lang';
  await page.click('.setting input[maxlength="80"]', { clickCount: 3 }); await page.type('.setting input[maxlength="80"]', name); await page.keyboard.press('Tab'); await sleep(800);
  await page.click('.setting input[maxlength="8"]', { clickCount: 3 }); await page.type('.setting input[maxlength="8"]', 'JWL'); await page.keyboard.press('Tab'); await sleep(1200);
  const stored = await api(page, 'GET', '/api/preferences');
  check(stored.author === name && stored.initials === 'JWL' && stored.theme === 'dark', 'preferences stored ' + JSON.stringify(stored));
  const overridden = await api(page, 'GET', '/api/settings/wordcraft');
  const wui = JSON.parse(overridden['wordcraft.ui'] || '{}');
  check(wui.author === name && wui.initials === 'JWL' && wui.theme === 'dark', 'override wrote WordCraft options ' + JSON.stringify({ author: wui.author, initials: wui.initials, theme: wui.theme }));

  check((await navLabels(page)).includes('Settings'), 'English nav before the change');
  await Promise.all([page.waitForNavigation({ waitUntil: 'networkidle0' }), page.select('select[aria-label="Interface language"]', 'de')]);
  await page.waitForSelector('.settings-page'); await sleep(800);
  const labels = await navLabels(page);
  check(labels.includes('Meine Dateien') && labels.includes('Einstellungen'), 'German shell after the change ' + JSON.stringify(labels));
  check(await page.evaluate(() => document.documentElement.lang) === 'de', '<html lang=de>');
  check(await page.$eval('select[aria-label="Sprache der Oberfläche"]', e => e.value) === 'de', 'the language list shows the stored choice');
  await page.screenshot({ path: OUT + '/settings-de.png' });

  // Nothing stored for WordCraft: the launch fills language, author and initials from Settings.
  await api(page, 'PUT', '/api/settings/wordcraft', {});
  await page.evaluate(() => Object.keys(localStorage).filter(k => k.startsWith('wordcraft')).forEach(k => localStorage.removeItem(k)));
  await page.goto(BASE + '/new/wordcraft', { waitUntil: 'networkidle0' });
  await page.waitForSelector('section.editor iframe'); await sleep(12000);
  const frame = page.frames().find(f => f.url().includes('/apps/wordcraft'));
  const ui = await frame.evaluate(() => JSON.parse(localStorage.getItem('wordcraft.ui') || '{}'));
  check(ui.language === 'de' && ui.author === name && ui.initials === 'JWL', 'WordCraft opened with ' + JSON.stringify({ language: ui.language, author: ui.author, initials: ui.initials, theme: ui.theme }));
  // WordCraft paints its UI on a canvas: the screenshot shows the German menus.
  await page.screenshot({ path: OUT + '/app-language.png' });
  const after = await api(page, 'GET', '/api/settings/wordcraft');
  check(JSON.parse(after['wordcraft.ui'] || '{}').language === 'de', 'filled options saved back for the user');

  // Leave the account as it was.
  await api(page, 'PUT', '/api/preferences', {});
  await browser.close();
  console.log(fails.length ? 'FAILED: ' + fails.join(' | ') : 'ALL OK');
  process.exit(fails.length ? 1 : 0);
})().catch(e => { console.error(e); process.exit(2); });
