import test from 'node:test';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';

test('WXT registers early detection and browser capture as content scripts', async () => {
  const wxtRoot = new URL('./', import.meta.resolve('wxt'));
  const { registerWxt } = await import(new URL('core/wxt.mjs', wxtRoot));
  const { findEntrypoints } = await import(new URL('core/utils/building/find-entrypoints.mjs', wxtRoot));
  await registerWxt('build', { root: fileURLToPath(new URL('../', import.meta.url)) });
  const entrypoints = await findEntrypoints();

  for (const name of ['content-early', 'web-media-capture']) {
    const entrypoint = entrypoints.find((entry) => entry.name === name);
    assert.ok(entrypoint, `Missing ${name} entrypoint`);
    assert.equal(entrypoint.type, 'content-script', `${name} must run automatically in webpages`);
    assert.equal(entrypoint.options.runAt, 'document_start');
    assert.equal(entrypoint.options.allFrames, true);
    assert.equal(entrypoint.options.matchAboutBlank, true);
    assert.deepEqual(entrypoint.options.matches, ['<all_urls>']);
  }
});

test('WXT packages the overlay font and its license for Chrome and Firefox', async () => {
  const wxtRoot = new URL('./', import.meta.resolve('wxt'));
  const wxtModule = await import(new URL('core/wxt.mjs', wxtRoot));
  const { getPublicFiles } = await import(new URL('core/utils/fs.mjs', wxtRoot));

  for (const browser of ['chrome', 'firefox']) {
    await wxtModule.registerWxt('build', {
      root: fileURLToPath(new URL('../', import.meta.url)), browser,
    });
    assert.equal(wxtModule.wxt.config.publicDir, fileURLToPath(new URL('../public', import.meta.url)));
    const files = (await getPublicFiles()).map(file => file.replaceAll('\\', '/'));
    assert.ok(files.includes('fonts/inter-latin-wght-normal.woff2'), `${browser} must package the overlay font`);
    assert.ok(files.includes('fonts/OFL.txt'), `${browser} must package the font license`);
  }
});
