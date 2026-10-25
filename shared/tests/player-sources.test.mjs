import test from 'node:test';
import assert from 'node:assert/strict';
import * as player from '../player-sources.mjs';
import { captureSource } from '../web-media-capture.mjs';

const options = { baseUrl: 'https://video.example/watch/42', title: 'Example clip' };

test('groups Video.js, JW Player and structured video sources by explicit player identity', () => {
  const groups = player.collectPlayerGroups({ ...options, videos: [
    { id: 'vjs', setup: '{"sources":[{"src":"/main.mp4?quality=720&token=a%2Bb","type":"video/mp4","label":"720p","fps":60}]}' },
    { id: 'jw' },
  ], scripts: [
    { text: 'jwplayer("jw").setup({playlist:[{sources:[{file:"/main-1080.mp4",label:"1080p"}], advertising:{file:"/ads/preroll.mp4"}}]});' },
    { type: 'application/ld+json', text: '{"@type":"VideoObject","contentUrl":"/structured.mp4"}' },
  ] });
  assert.deepEqual(groups.map((g) => g.playerId), ['vjs', 'jw']);
  assert.equal(groups[0].sources[0].url, 'https://video.example/main.mp4?quality=720&token=a%2Bb');
  assert.equal(groups[0].sources[0].fps, 60);
  assert.deepEqual(groups[1].sources.map((s) => s.url), ['https://video.example/main-1080.mp4']);
  assert.deepEqual(player.collectPlayerGroups({ ...options, videos: [{ id: 'only' }], scripts: [
    { type: 'application/ld+json', text: '{"@graph":[{"@type":"VideoObject","contentUrl":"/structured.mp4"}]}' },
  ] })[0].sources.map((s) => s.url), ['https://video.example/structured.mp4']);
});

test('does not attach page-wide configuration to one of several unrelated players or evaluate expressions', () => {
  const groups = player.collectPlayerGroups({ ...options, videos: [{ id: 'a' }, { id: 'b' }], scripts: [
    { text: 'var flashvars={video_url:"/main.mp4"};' },
    { text: 'jwplayer("a").setup({sources:[{file:"/unsafe.mp4"+sideEffect()}]});' },
  ] });
  assert.deepEqual(groups.map((g) => g.sources), [[], []]);
});

test('HTML5 sources remain discoverable before preload or playback, with quality labels', () => {
  const sources = player.collectPlayerSources({ ...options, videos: [{ readyState: 0, sources: [
    { src: '/get_file/hash/42_720m.mp4/?token=a%2Bb&quality=720', type: 'video/mp4', label: '720p' },
    { src: '/get_file/hash/42_480m.mp4/', type: 'video/mp4', label: '480p' },
  ] }] });
  assert.deepEqual(sources.map(({ url, title, protocol }) => ({ url, title, protocol })), [
    { url: 'https://video.example/get_file/hash/42_720m.mp4/?token=a%2Bb&quality=720', title: 'Example clip · 720p', protocol: 'direct' },
    { url: 'https://video.example/get_file/hash/42_480m.mp4/', title: 'Example clip · 480p', protocol: 'direct' },
  ]);
});

test('KVS flashvars associates every alternate URL with its matching quality, preserving signed queries', () => {
  const sources = player.collectPlayerSources({ ...options, scripts: [String.raw`
    var flashvars = {
      video_id: '42', video_title: 'Example clip',
      video_url: 'https:\/\/cdn.example\/get_file\/42.mp4\/?v-acctoken=a%2Bb\u0026quality=1080',
      video_url_text: '1080p',
      video_alt_url: '//cdn.example/get_file/42_720p.mp4/?v-acctoken=next',
      video_alt_url_text: '720p',
      video_alt_url2: '/get_file/42_360p.mp4/', video_alt_url2_text: '360p',
      preview_url: 'https://cdn.example/poster.jpg', video_url_hd: '1'
    };`] });
  assert.deepEqual(sources.map(({ url, title }) => ({ url, title })), [
    { url: 'https://cdn.example/get_file/42.mp4/?v-acctoken=a%2Bb&quality=1080', title: 'Example clip · 1080p' },
    { url: 'https://cdn.example/get_file/42_720p.mp4/?v-acctoken=next', title: 'Example clip · 720p' },
    { url: 'https://video.example/get_file/42_360p.mp4/', title: 'Example clip · 360p' },
  ]);
});

test('player config is parsed as data without executing expressions or accepting obfuscated URLs', () => {
  globalThis.playerSourceExecuted = false;
  const sources = player.collectPlayerSources({ ...options, scripts: [String.raw`
    var unrelated = { video_url: 'https://cdn.example/unrelated.mp4' };
    var flashvars = {
      video_url: 'function/0/https://cdn.example/get_file/encoded.mp4',
      video_alt_url: (() => { globalThis.playerSourceExecuted = true; return 'https://cdn.example/unsafe.mp4'; })(),
      video_alt_url2: 'https://cdn.example/prefix.mp4' + 'suffix',
      video_alt_url3: 'https://cdn.example/real.mp4', video_alt_url3_text: '720p'
    };
    var other = {video_url: 'https://cdn.example/other.mp4'};`] });
  assert.equal(globalThis.playerSourceExecuted, false);
  assert.deepEqual(sources.map((source) => source.url), ['https://cdn.example/real.mp4']);
  delete globalThis.playerSourceExecuted;
});

test('DOM sources deduplicate currentSrc, reject non-video/ad resources, and retain HLS/DASH', () => {
  const sources = player.collectPlayerSources({ ...options, videos: [{
    currentSrc: 'https://cdn.example/video.mp4', src: 'https://cdn.example/video.mp4', sources: [
      { src: 'https://cdn.example/video.mp4', label: '720p' },
      { src: 'https://cdn.example/manifest', type: 'application/dash+xml' },
      { src: 'https://cdn.example/master.m3u8/' },
      { src: 'javascript:globalThis.playerSourceExecuted=true', type: 'video/mp4' },
      { src: 'https://cdn.example/ad/promo.mp4', type: 'video/mp4' },
      { src: 'https://cdn.example/audio.mp3/', type: 'video/mp4' },
    ],
  }] });
  assert.deepEqual(sources.map((source) => source.protocol), ['direct', 'dash', 'hls']);
  assert.equal(sources[0].title, 'Example clip · 720p');
});

test('media endpoints with a trailing slash are recognized without modifying their path or token', () => {
  for (const [path, protocol] of [['clip.mp4/', 'direct'], ['master.m3u8/', 'hls'], ['manifest.mpd/', 'dash']]) {
    const url = `https://cdn.example/get_file/${path}?token=a%2Bb`;
    assert.equal(captureSource({ url })?.protocol, protocol);
    assert.equal(captureSource({ url })?.url, url);
  }
  assert.equal(captureSource({ url: 'https://cdn.example/segment.m4s/', mime: 'video/mp4' }), null);
});

test('DOM quality sources retain frame rates and do not collapse signed selectors', () => {
  const group = player.collectPlayerGroups({ ...options, videos: [{ id: 'v', sources: [
    { src: '/main.mp4?fps=30', label: '720p', fps: 30 },
    { src: '/main.mp4?fps=60', label: '720p', fps: 60 },
  ] }] })[0];
  assert.deepEqual(group.sources.map((source) => source.fps), [30, 60]);
});

test('duplicate player IDs in separate DOM roots do not steal each other’s configuration', () => {
  const groups = player.collectPlayerGroups({ ...options, videos: [
    { id: 'same', configId: 'same' }, { id: 'unique-in-shadow', configId: 'same' },
  ], scripts: [{ text: 'jwplayer("same").setup({sources:[{file:"/unknown-owner.mp4"}]});' }] });
  assert.deepEqual(groups.map((group) => group.sources), [[], []]);
});
