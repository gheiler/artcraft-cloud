// CalendarCraft end to end against a real Stalwart (docs/calendar.md): a person with a mailbox
// opens CalendarCraft, which finds their calendar through `/caldav/`; an event typed in the app is
// on the server (checked with curl PROPFIND/REPORT straight at Stalwart, as the person, with their
// artcraftCloud password); an event put on the server with curl shows in the app; and an `.ics`
// file in the drive opens in CalendarCraft and is saved back with a new event.
//
// Needs: artcraftCloud at BASE with CalendarCraft installed (`artcraft-cloud apps install
// calendarcraft <zip> --version <v>`), mail set up, and USER/PASS a person with a mailbox.
// STALWART is Stalwart's HTTP address as seen from here (for the curl checks).
const puppeteer = require('puppeteer-core');
const { spawnSync } = require('child_process');
const sleep = ms => new Promise(r => setTimeout(r, ms));
const BASE = process.env.BASE || 'http://127.0.0.1:8090';
const STALWART = process.env.STALWART || 'http://127.0.0.1:8091';
const OUT = process.env.OUT || '/workspace/calendarcraft-shots';
const USER = process.env.E2E_USER || 'cara', PASS = process.env.E2E_PASS || 'pass-cara-12345';
const ADDRESS = process.env.E2E_ADDRESS || `${USER}@example.test`;
const fails = [];
const check = (ok, what) => { console.log((ok ? 'ok   ' : 'FAIL ') + what); if (!ok) fails.push(what); };
const home = `${STALWART}/dav/cal/${encodeURIComponent(ADDRESS)}/`;
const curl = (args, body) => {
  const r = spawnSync('curl', ['-s', '--max-time', '15', '-u', `${ADDRESS}:${PASS}`, ...args, ...(body ? ['--data-binary', body] : [])], { encoding: 'utf8' });
  return r.stdout || '';
};
const api = (page, method, url, body, raw) => page.evaluate(async (method, url, body, raw) => {
  const r = await fetch(url, { method, headers: { 'x-requested-with': 'artcraft', 'content-type': raw ? 'text/calendar' : 'application/json' }, body: body === undefined ? undefined : (raw ? body : JSON.stringify(body)) });
  const text = await r.text(); let json; try { json = JSON.parse(text); } catch { json = text; } return { status: r.status, json };
}, method, url, body, raw);
const ymd = d => d.toISOString().slice(0, 10).replace(/-/g, '');

async function appFrame(page) {
  await page.waitForSelector('section.editor iframe', { timeout: 20000 });
  const handle = await page.$('section.editor iframe');
  const frame = await handle.contentFrame();
  await frame.waitForSelector('#calendarcraft_canvas', { timeout: 20000 });
  await sleep(6000);
  return { frame, handle };
}

async function typeEvent(page, handle, sentence) {
  const box = await handle.boundingBox();
  await page.mouse.click(box.x + box.width - 40, box.y + box.height - 40);
  await sleep(300);
  // N opens the new-event dialog (browsers keep Ctrl+N for a new window).
  await page.keyboard.press('KeyN');
  await sleep(800);
  await page.keyboard.type(sentence, { delay: 20 });
  await sleep(500);
  await page.keyboard.press('Enter');
  await sleep(4000);
}

(async () => {
  const browser = await puppeteer.launch({ executablePath: '/usr/bin/google-chrome', headless: 'new', args: ['--no-sandbox', '--use-angle=swiftshader', '--enable-unsafe-swiftshader', '--window-size=1440,900', '--lang=en-US'] });
  const page = await browser.newPage();
  await page.setViewport({ width: 1440, height: 900 });
  page.on('pageerror', e => fails.push('pageerror ' + e.message.slice(0, 200)));
  await page.goto(BASE + '/', { waitUntil: 'networkidle0' });
  await page.type('input[autocomplete=username]', USER); await page.type('input[type=password]', PASS);
  await page.click('button[type=submit]'); await page.waitForSelector('.sidebar nav');
  await api(page, 'PUT', '/api/preferences', { theme: 'light', language: 'en' });

  // 1. The app finds the person's calendar on the mail server.
  await page.goto(BASE + '/new/calendarcraft', { waitUntil: 'networkidle0' });
  let { frame, handle } = await appFrame(page);
  const host = await frame.evaluate(() => window.calendarcraftHost);
  check(host && host.accounts.length === 1 && host.accounts[0].url === `/caldav/${encodeURIComponent(ADDRESS)}/`, 'the hook names the calendar home: ' + JSON.stringify(host && host.accounts));
  check(await frame.evaluate(() => typeof window.calendarcraft?.execute === 'function'), 'the page API is there for the host');

  // 2. An event typed in CalendarCraft goes to Stalwart.
  const title = `Planning ${Date.now() % 100000}`;
  await typeEvent(page, handle, `${title} tomorrow 3pm`);
  await page.screenshot({ path: `${OUT}/artcraft-month-light.png` });
  const tomorrow = new Date(Date.now() + 86400000);
  const report = curl(['-X', 'REPORT', '-H', 'Depth: 1', '-H', 'Content-Type: application/xml', home + 'default/'],
    `<c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:getetag/><c:calendar-data/></d:prop><c:filter><c:comp-filter name="VCALENDAR"><c:comp-filter name="VEVENT"><c:time-range start="${ymd(tomorrow)}T000000Z" end="${ymd(new Date(Date.now() + 2 * 86400000))}T235959Z"/></c:comp-filter></c:comp-filter></c:filter></c:calendar-query>`);
  check(report.includes(`SUMMARY:${title}`), `curl REPORT finds "${title}" on Stalwart`);
  check(/DTSTART[^:\r\n]*:\d{8}T\d{6}/.test(report), 'with a start time');
  const propfind = curl(['-X', 'PROPFIND', '-H', 'Depth: 1', home], '<d:propfind xmlns:d="DAV:"><d:prop><d:displayname/><d:resourcetype/></d:prop></d:propfind>');
  check(propfind.includes('/default/'), 'curl PROPFIND lists the default calendar');

  // 3. An event put on the server shows in the app after a refresh.
  const uid = `curl-${Date.now()}`;
  const day = new Date(Date.now() + 2 * 86400000);
  const ics = `BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//e2e//EN\r\nBEGIN:VEVENT\r\nUID:${uid}\r\nDTSTAMP:${ymd(new Date())}T000000Z\r\nDTSTART:${ymd(day)}T090000Z\r\nDTEND:${ymd(day)}T100000Z\r\nSUMMARY:Put with curl\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n`;
  spawnSync('curl', ['-s', '-u', `${ADDRESS}:${PASS}`, '-X', 'PUT', '-H', 'Content-Type: text/calendar', '--data-binary', ics, `${home}default/${uid}.ics`]);
  await frame.evaluate(() => window.calendarcraft.execute('sync.refresh'));
  await sleep(4000);
  await page.screenshot({ path: `${OUT}/artcraft-after-refresh.png` });
  const state = JSON.parse(await frame.evaluate(() => window.calendarcraft.state || '{}'));
  check((state.events || []).some(e => e.title === 'Put with curl'), 'the app lists the event put with curl');
  check((state.events || []).some(e => e.title === title), 'and the one typed in it');

  // 3b. A public .ics feed on another server comes in through /api/calendar/remote (the page
  // itself may only fetch this origin). FEED_URL may be any public feed.
  const FEED = process.env.FEED_URL || 'https://calendar.google.com/calendar/ical/en.german%23holiday%40group.v.calendar.google.com/public/basic.ics';
  await frame.evaluate(url => window.calendarcraft.execute('calendar.subscribe', { url, name: 'Holidays' }), FEED);
  await sleep(6000);
  const subscribed = JSON.parse(await frame.evaluate(() => window.calendarcraft.state || '{}'));
  const feed = (subscribed.calendars || []).find(c => c.name === 'Holidays');
  const feedEvents = feed ? (subscribed.events || []).filter(e => e.calendar === feed.id).length : 0;
  check(feedEvents > 0, `the subscribed feed has ${feedEvents} events ` + JSON.stringify(subscribed.errors || {}));
  await page.screenshot({ path: `${OUT}/artcraft-subscribed.png` });

  // 4. Dark, from artcraftCloud's Settings.
  await api(page, 'PUT', '/api/preferences', { theme: 'dark', language: 'en' });
  await page.goto(BASE + '/new/calendarcraft', { waitUntil: 'networkidle0' });
  ({ frame, handle } = await appFrame(page));
  await page.screenshot({ path: `${OUT}/artcraft-month-dark.png` });
  await api(page, 'PUT', '/api/preferences', { theme: 'light', language: 'en' });

  // 5. An .ics file in the drive opens in CalendarCraft and is saved back.
  const fileIcs = `BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//e2e//EN\r\nX-WR-CALNAME:Team holidays\r\nBEGIN:VEVENT\r\nUID:h1@e2e\r\nDTSTAMP:20261101T000000Z\r\nDTSTART;VALUE=DATE:${ymd(day)}\r\nDTEND;VALUE=DATE:${ymd(new Date(day.getTime() + 2 * 86400000))}\r\nSUMMARY:Company retreat\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n`;
  const fileName = `Team holidays ${Date.now() % 100000}.ics`;
  const put = await api(page, 'PUT', '/api/files/content?path=' + encodeURIComponent(fileName), fileIcs, true);
  check(put.status === 200 || put.status === 201, 'the .ics is in the drive: ' + put.status);
  await page.goto(BASE + '/open/' + encodeURIComponent(fileName), { waitUntil: 'networkidle0' });
  ({ frame, handle } = await appFrame(page));
  await page.screenshot({ path: `${OUT}/artcraft-ics-open.png` });
  await typeEvent(page, handle, 'Kickoff day after tomorrow 10am');
  await frame.evaluate(() => window.calendarcraft.execute('file.save'));
  await sleep(3000);
  const saved = await api(page, 'GET', '/api/files/content?path=' + encodeURIComponent(fileName));
  const text = typeof saved.json === 'string' ? saved.json : JSON.stringify(saved.json);
  check(text.includes('SUMMARY:Company retreat') && text.includes('SUMMARY:Kickoff'), 'the file was saved back with the new event');
  check(!text.includes(title), 'the server event did not leak into the file');

  // Clean up the server events.
  spawnSync('curl', ['-s', '-u', `${ADDRESS}:${PASS}`, '-X', 'DELETE', `${home}default/${uid}.ics`]);
  await browser.close();
  console.log(fails.length ? `\n${fails.length} FAILED` : '\nall passed');
  process.exit(fails.length ? 1 : 0);
})().catch(e => { console.error(e); process.exit(1); });
