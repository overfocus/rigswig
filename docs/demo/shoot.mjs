// Screenshots of the audit on the page, from the invented setup make-demo.sh builds. Nothing personal is in them: the
// page reads only the demo home folder and its repos.
//
//   docs/demo/make-demo.sh /tmp/rigswig-demo
//   cargo build --release
//   PLAYWRIGHT_CORE=/path/to/node_modules/playwright-core node docs/demo/shoot.mjs /tmp/rigswig-demo target/release/rigswig docs/images
//
// Needs Google Chrome (or CHROME=<path>) and playwright-core (any install; PLAYWRIGHT_CORE points at it). Writes
// audit.png (one repo's findings) and audit-fleet.png (fleet mode across ~/code/*), 1280x900 at 2x.
import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';
import { join, resolve } from 'node:path';

const [root, bin, out] = process.argv.slice(2).map((p) => p && resolve(p));
if (!root || !bin || !out) {
  console.error('usage: node docs/demo/shoot.mjs <demo folder> <rigswig binary> <output folder>');
  process.exit(2);
}
const require = createRequire(import.meta.url);
const { chromium } = require(process.env.PLAYWRIGHT_CORE || 'playwright-core');
const home = join(root, 'home');

// The page, on the demo home folder only (no environment variables of this machine that rigswig reads).
const server = spawn(bin, ['ui', '--repo', join(home, 'code/acme-shop'), '--allow-github', '--idle', '5'], {
  env: { PATH: process.env.PATH, HOME: home, USERPROFILE: home },
});
const url = await new Promise((ok, fail) => {
  let buf = '';
  server.stdout.on('data', (d) => {
    buf += d;
    const m = buf.match(/http:\/\/127\.0\.0\.1:\d+\/\?t=[0-9a-f]+/);
    if (m) ok(m[0]);
  });
  server.on('exit', (c) => fail(new Error(`rigswig ui exited ${c}`)));
});

const browser = await chromium.launch(
  process.env.CHROME ? { executablePath: process.env.CHROME } : { channel: 'chrome' },
);
try {
  // Reduced motion: no card is caught halfway through opening.
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 }, deviceScaleFactor: 2, reducedMotion: 'reduce' });
  const done = () => page.waitForSelector('.prog.done', { timeout: 60_000 });
  const top = (sel) => page.evaluate((s) => {
    const el = document.querySelector(s);
    window.scrollTo({ top: el.getBoundingClientRect().top + window.scrollY - 12, behavior: 'instant' });
  }, sel);

  await page.goto(url);
  await page.getByRole('button', { name: 'Audit this repo' }).click();
  await done();
  // The first high finding opens by itself; open the plugin one too, to show a saving and its check.
  await page.locator('details.finding', { hasText: 'terraform-kit' }).evaluate((d) => { d.open = true; });
  await top('.audit');
  await page.mouse.move(0, 0);
  await page.screenshot({ path: join(out, 'audit.png') });

  await page.fill('#fleet-in', '~/code/*');
  await page.getByRole('button', { name: 'Audit many repos' }).click();
  await done();
  await top('.audit');
  await page.mouse.move(0, 0);
  await page.screenshot({ path: join(out, 'audit-fleet.png') });
  console.log(`wrote ${join(out, 'audit.png')} and ${join(out, 'audit-fleet.png')}`);
} finally {
  await browser.close();
  server.kill();
}
