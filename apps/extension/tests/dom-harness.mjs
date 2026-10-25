import { registerHooks } from 'node:module';
import { readFileSync, existsSync, readdirSync } from 'node:fs';
import ts from 'typescript';

registerHooks({
  resolve(specifier, context, next) {
    if (specifier.startsWith('.') && !/\.[cm]?[jt]s$/.test(specifier) && context.parentURL?.startsWith('file:')) {
      const candidate = new URL(specifier + '.ts', context.parentURL);
      if (existsSync(candidate)) return { url: candidate.href, shortCircuit: true };
    }
    return next(specifier, context);
  },
  load(url, context, next) {
    if (url.startsWith('file:') && url.endsWith('.ts') && url.includes('/apps/extension/src/')) {
      return { format: 'module', source: ts.transpileModule(readFileSync(new URL(url), 'utf8'), {
        compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext },
      }).outputText, shortCircuit: true };
    }
    return next(url, context);
  },
});

// WXT's existing dependency graph supplies this DOM implementation; no browser or
// application build is needed to exercise the real detector/overlay adapters.
const packages = new URL('../../../node_modules/.pnpm/', import.meta.url);
const domPackage = readdirSync(packages).find((name) => name.startsWith('linkedom@'));
const { parseHTML } = await import(new URL(domPackage + '/node_modules/linkedom/esm/index.js', packages));

export function domHarness(context, html, url = 'https://video.example/watch/42') {
  context.mock.timers.enable({ apis: ['setTimeout', 'setInterval'] });
  const { window, document } = parseHTML('<!doctype html><html><head><title>Video</title></head><body>' + html + '</body></html>');
  const originals = new Map();
  const set = (key, value) => {
    originals.set(key, Object.getOwnPropertyDescriptor(globalThis, key));
    Object.defineProperty(globalThis, key, { configurable: true, writable: true, value });
  };
  Object.defineProperty(document, 'baseURI', { configurable: true, value: url });
  Object.defineProperty(document, 'visibilityState', { configurable: true, writable: true, value: 'visible' });
  const location = new URL(url);
  for (const [key, value] of Object.entries({ window, document, location, Element: window.Element,
    HTMLElement: window.HTMLElement, HTMLVideoElement: window.HTMLVideoElement, ShadowRoot: window.ShadowRoot,
    innerWidth: 1400, innerHeight: 900, getComputedStyle: () => ({ display: 'block', visibility: 'visible', opacity: '1' }),
    ResizeObserver: class { observe() {} unobserve() {} disconnect() {} },
    IntersectionObserver: class {
      constructor(callback) { this.callback = callback; }
      observe(target) { this.callback([{ target, isIntersecting: true }]); }
      unobserve() {} disconnect() {}
    },
    MutationObserver: class { observe() {} disconnect() {} },
    requestAnimationFrame: (callback) => { callback(); return 1; }, cancelAnimationFrame: () => {},
    browser: { runtime: { getURL: (path) => 'chrome-extension://test/' + path } },
  })) set(key, value);
  window.innerWidth = 1400; window.innerHeight = 900;
  const rect = { x: 20, y: 20, left: 20, top: 20, right: 660, bottom: 380, width: 640, height: 360 };
  window.HTMLElement.prototype.getBoundingClientRect = () => rect;
  const configure = (root = document) => {
    for (const video of root.querySelectorAll('video')) {
      video.currentSrc = video.getAttribute('src') ? new URL(video.getAttribute('src'), location.href).href : '';
      video.src = video.currentSrc;
      video.readyState = 0; video.duration = 3; video.loop = true; video.muted = true; video.autoplay = true;
    }
    for (const source of root.querySelectorAll('source')) {
      source.src = new URL(source.getAttribute('src'), location.href).href;
      source.type = source.getAttribute('type') || '';
    }
  };
  configure();
  context.after(() => { for (const [key, original] of originals) {
    if (original) Object.defineProperty(globalThis, key, original); else delete globalThis[key];
  } });
  return { window, document, location, configure, tick: () => {
    context.mock.timers.tick(1000); context.mock.timers.tick(150);
  } };
}
