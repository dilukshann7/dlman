import test from 'node:test';
import assert from 'node:assert/strict';
import { parseYoutubeVerificationUrl, youtubeSessionCookies, shareYoutubeSession } from '../youtube-verification.mjs';

const token = 'd7430c5b-3b71-4db5-b7a6-1aa0cfec395d';
const url = 'https://www.youtube.com/watch?v=HLUamwXQ218';
const verificationUrl = `${url}#dlman-verify=${token}`;
const cookie = { domain: '.youtube.com', path: '/', name: 'SID', value: 'session', secure: true, httpOnly: true, hostOnly: false };

test('verification links accept only canonical HTTPS YouTube videos with UUID tokens', () => {
  assert.deepEqual(parseYoutubeVerificationUrl(`${url}#dlman-verify=${token}`), { token, url });
  for (const bad of [
    `http://www.youtube.com/watch?v=HLUamwXQ218#dlman-verify=${token}`,
    `https://evil.example/watch?v=HLUamwXQ218#dlman-verify=${token}`,
    `${url}#dlman-verify=invalid`, `${url}&list=abc#other=${token}`,
  ]) assert.equal(parseYoutubeVerificationUrl(bad), null);
});

test('cookie sharing includes only applicable YouTube cookies and strips extra browser fields', () => {
  assert.deepEqual(youtubeSessionCookies([
    { ...cookie, storeId: 'private', extra: 'discard' },
    { ...cookie, domain: '.google.com' },
    { ...cookie, domain: '.evilyoutube.com' },
    { ...cookie, expirationDate: 1 },
  ], 2000), [{ domain: '.youtube.com', path: '/', name: 'SID', value: 'session', secure: true, http_only: true, host_only: false, expires: null }]);
  assert.throws(() => youtubeSessionCookies([{ ...cookie, value: 'injected\nrow' }]), /Invalid/);
  assert.throws(() => youtubeSessionCookies([]), /No YouTube/);
  assert.throws(() => youtubeSessionCookies(Array(257).fill(cookie)), /Too many/);
});

test('sharing validates the tab before reading cookies and after asynchronous reads', async () => {
  const calls = [];
  const request = { token, url, tabId: 7, created: 1000 };
  const result = await shareYoutubeSession(request, {
    now: () => 2000,
    getTab: async (id) => { calls.push(['tab', id]); return { url: verificationUrl, cookieStoreId: 'firefox-container-2' }; },
    getCookies: async (details) => { calls.push(['cookies', details]); return [cookie]; },
    submit: async (payload) => { calls.push(['submit', payload]); return { success: true }; },
  });
  assert.deepEqual(result, { success: true });
  assert.deepEqual(calls.map(([name]) => name), ['tab', 'cookies', 'tab', 'submit']);
  assert.deepEqual(calls[1][1], { url, storeId: 'firefox-container-2' });
  assert.equal(calls[3][1].token, token);
  assert.equal(calls[3][1].url, url);
  assert.equal(calls[3][1].cookies[0].value, 'session');
});

test('expired, private and navigated tabs never submit a session', async () => {
  for (const scenario of ['expired', 'private', 'navigated', 'changed-during-read']) {
    let reads = 0, submissions = 0, tabReads = 0;
    await assert.rejects(shareYoutubeSession({ token, url, tabId: 7, created: scenario === 'expired' ? 0 : 1_000_000 }, {
      now: () => 1_000_001,
      getTab: async () => ({ incognito: scenario === 'private', url: scenario === 'navigated' || (scenario === 'changed-during-read' && tabReads++ > 0) ? 'https://www.youtube.com/watch?v=abcdefghijk' : verificationUrl }),
      getCookies: async () => { reads++; return [cookie]; },
      submit: async () => { submissions++; return { success: true }; },
    }));
    assert.equal(submissions, 0, scenario);
    assert.equal(reads, scenario === 'changed-during-read' ? 1 : 0, scenario);
  }
});

test('removing or replacing a verification fragment revokes the stale approval', async () => {
  for (const changed of [url, `${url}#dlman-verify=d7430c5b-3b71-4db5-b7a6-1aa0cfec395e`]) {
    let submissions = 0;
    await assert.rejects(shareYoutubeSession({ token, url, tabId: 7, created: 1000 }, {
      now: () => 2000,
      getTab: async () => ({ url: changed }),
      getCookies: async () => [cookie],
      submit: async () => { submissions++; },
    }), /verification|Verification/);
    assert.equal(submissions, 0);
  }
});
