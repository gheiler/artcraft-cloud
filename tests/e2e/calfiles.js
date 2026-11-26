// File-backed calendars in CalendarCraft (host mode): create a calendar as an .ics
// in Calendars/, reload after an external change, and show the conflict dialog when
// both the file and this session have edits.
//
// Needs: artcraftCloud at BASE with a CalendarCraft build that speaks calendarcraft:file
// (`artcraft-cloud apps install calendarcraft <zip> --version <v>`), and USER/PASS.
const puppeteer = require('puppeteer-core');
const sleep = ms => new Promise(r => setTimeout(r, ms));
const BASE = process.env.BASE || 'http://127.0.0.1:8094';
const OUT = process.env.OUT || '/workspace/artcraft-cloud-shots';
const USER = process.env.E2E_USER || 'admin';
const PASS = process.env.E2E_PASS || 'admin-pass-12345';
const fails = [];
const check = (ok, what) => { console.log((ok ? 'ok   ' : 'FAIL ') + what); if (!ok) fails.push(what); };

const api = (page, method, url, body, raw, extraHeaders) => page.evaluate(async (method, url, body, raw, extraHeaders) => {
  const headers = { 'x-requested-with': 'artcraft', 'content-type': raw ? 'text/calendar' : 'application/json', ...(extraHeaders || {}) };
  const r = await fetch(url, { method, headers, body: body === undefined ? undefined : (raw ? body : JSON.stringify(body)) });
  const text = await r.text();
  const etag = r.headers.get('etag');
  let json; try { json = JSON.parse(text); } catch { json = text; }
  return { status: r.status, json, text, etag };
}, method, url, body, raw, extraHeaders || null);

async function appFrame(page) {
  await page.waitForSelector('section.editor iframe', { timeout: 20000 });
  const handle = await page.$('section.editor iframe');
  const frame = await handle.contentFrame();
  await frame.waitForSelector('#calendarcraft_canvas', { timeout: 20000 });
  await sleep(4000);
  return { frame, handle };
}

async function stateOf(frame) {
  const raw = await frame.evaluate(() => window.calendarcraft && window.calendarcraft.state || '{}');
  try { return JSON.parse(raw); } catch { return {}; }
}

async function waitFor(frame, pred, timeout = 15000, step = 400) {
  const start = Date.now();
  let last;
  while (Date.now() - start < timeout) {
    last = await stateOf(frame);
    if (pred(last)) return last;
    await sleep(step);
  }
  return last;
}

(async () => {
  const fs = require('fs');
  fs.mkdirSync(OUT, { recursive: true });
  const browser = await puppeteer.launch({
    executablePath: process.env.CHROME || '/usr/bin/google-chrome',
    headless: 'new',
    args: ['--no-sandbox', '--use-angle=swiftshader', '--enable-unsafe-swiftshader', '--window-size=1440,900', '--lang=en-US'],
  });
  const page = await browser.newPage();
  await page.setViewport({ width: 1440, height: 900 });
  page.on('pageerror', e => fails.push('pageerror ' + e.message.slice(0, 200)));

  await page.goto(BASE + '/', { waitUntil: 'networkidle0' });
  await page.type('input[autocomplete=username]', USER);
  await page.type('input[type=password]', PASS);
  await page.click('button[type=submit]');
  await page.waitForSelector('.sidebar nav');
  await api(page, 'PUT', '/api/preferences', { theme: 'light', language: 'en' });

  await page.goto(BASE + '/new/calendarcraft', { waitUntil: 'networkidle0' });
  let { frame } = await appFrame(page);
  check(await frame.evaluate(() => typeof window.calendarcraft?.execute === 'function'), 'the page API is there');

  const calName = `Team plans ${Date.now() % 100000}`;
  await frame.evaluate(name => window.calendarcraft.execute('calendar.create', { name }), calName);
  const created = await waitFor(frame, s => (s.fileCalendars || []).some(c => (c.path || '').includes('Calendars/') && (c.name || '').startsWith('Team plans') || (s.calendars || []).some(c => c.name === calName)));
  const fileCal = (created.fileCalendars || []).find(c => (c.path || '').startsWith('Calendars/'))
    || (created.fileCalendars || [])[0];
  check(!!fileCal && !!(fileCal.path), 'created a file-backed calendar: ' + JSON.stringify(created.fileCalendars || created.calendars));
  const path = fileCal && fileCal.path;
  const listed = path ? await api(page, 'GET', '/api/files/stat?path=' + encodeURIComponent(path)) : { status: 0 };
  check(listed.status === 200 && listed.json && listed.json.etag, 'the .ics is in the drive with an etag: ' + listed.status);
  await page.screenshot({ path: `${OUT}/calfiles-created.png` });

  // External change, no local edits: the calendar should reload the new event.
  const today = new Date();
  const ymd = d => d.toISOString().slice(0, 10).replace(/-/g, '');
  const day = new Date(today.getTime() + 2 * 86400000);
  const ics = `BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//e2e//EN\r\nX-WR-CALNAME:${calName}\r\nBEGIN:VEVENT\r\nUID:ext-${Date.now()}\r\nDTSTAMP:${ymd(today)}T000000Z\r\nDTSTART;VALUE=DATE:${ymd(day)}\r\nDTEND;VALUE=DATE:${ymd(new Date(day.getTime() + 86400000))}\r\nSUMMARY:External planning day\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n`;
  const etag = (listed.json && listed.json.etag) || (fileCal && fileCal.etag);
  const put = await api(page, 'PUT', '/api/files/content?path=' + encodeURIComponent(path), ics, true, { 'If-Match': '"' + String(etag).replace(/"/g, '') + '"' });
  check(put.status === 200, 'external write succeeded: ' + put.status + ' ' + JSON.stringify(put.json).slice(0, 180));
  await frame.evaluate(() => window.calendarcraft.execute('calendar.syncFiles'));
  const reloaded = await waitFor(frame, s => (s.events || []).some(e => e.title === 'External planning day'));
  check((reloaded.events || []).some(e => e.title === 'External planning day'), 'reloaded the external event after sync');
  await page.screenshot({ path: `${OUT}/calfiles-reloaded.png` });

  // Local unsaved edit + another external change => conflict dialog.
  const after = await api(page, 'GET', '/api/files/stat?path=' + encodeURIComponent(path));
  const calId = fileCal.id || ((reloaded.fileCalendars || [])[0] || {}).id;
  await frame.evaluate((title, calendar, date) => {
    window.calendarcraft.execute('event.add', { title, calendar, date });
  }, 'My unsaved note', calId, day.toISOString().slice(0, 10));
  await sleep(400);
  const conflictIcs = `BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//e2e//EN\r\nX-WR-CALNAME:${calName}\r\nBEGIN:VEVENT\r\nUID:ext2-${Date.now()}\r\nDTSTAMP:${ymd(today)}T000000Z\r\nDTSTART;VALUE=DATE:${ymd(day)}\r\nDTEND;VALUE=DATE:${ymd(new Date(day.getTime() + 86400000))}\r\nSUMMARY:Second external change\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n`;
  const etag2 = after.json && after.json.etag;
  const put2 = await api(page, 'PUT', '/api/files/content?path=' + encodeURIComponent(path), conflictIcs, true, { 'If-Match': '"' + String(etag2).replace(/"/g, '') + '"' });
  check(put2.status === 200, 'second external write succeeded: ' + put2.status);
  await frame.evaluate(() => window.calendarcraft.execute('calendar.syncFiles'));
  const conflicted = await waitFor(frame, s => !!(s.conflict && s.conflict.path), 12000);
  check(!!(conflicted.conflict && conflicted.conflict.path), 'conflict dialog state is set: ' + JSON.stringify(conflicted.conflict));
  await sleep(800);
  await page.screenshot({ path: `${OUT}/calfiles-conflict.png` });

  await frame.evaluate(() => window.calendarcraft.execute('calendar.resolveConflict', { choice: 'theirs' }));
  const resolved = await waitFor(frame, s => !s.conflict && (s.events || []).some(e => e.title === 'Second external change'), 12000);
  check(!resolved.conflict, 'conflict cleared after reload theirs');
  check((resolved.events || []).some(e => e.title === 'Second external change'), 'theirs version is shown');
  check(!(resolved.events || []).some(e => e.title === 'My unsaved note'), 'the unsaved local event was discarded');

  await browser.close();
  console.log(fails.length ? `\n${fails.length} FAILED` : '\nall passed');
  process.exit(fails.length ? 1 : 0);
})().catch(e => { console.error(e); process.exit(1); });
