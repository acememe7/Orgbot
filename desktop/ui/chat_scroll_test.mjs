// Run: node desktop/ui/chat_scroll_test.mjs (uses installed Chrome, no npm deps).
import {spawn} from 'node:child_process';
import {mkdtemp, readFile, rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import assert from 'node:assert/strict';
const profile = await mkdtemp(join(tmpdir(), 'orgbot-chat-'));
const chrome = spawn('google-chrome', ['--headless=new', '--no-sandbox', '--disable-gpu',
  '--no-first-run', '--remote-debugging-port=0', `--user-data-dir=${profile}`,
  new URL('./index.html', import.meta.url).href], {stdio: 'ignore'});
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
let socket;
try {
  let port;
  for (let i = 0; i < 100; i++) {
    try { port = (await readFile(join(profile, 'DevToolsActivePort'), 'utf8')).split('\n')[0]; break; } catch { await delay(100); }
  }
  assert.ok(port, 'Chrome did not start');
  const tabs = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
  socket = new WebSocket(tabs.find(t => t.type === 'page').webSocketDebuggerUrl);
  await new Promise(resolve => socket.addEventListener('open', resolve, {once: true}));
  let seq = 0;
  const pending = new Map();
  socket.addEventListener('message', ({data}) => {
    const response = JSON.parse(data);
    if (pending.has(response.id)) { pending.get(response.id)(response); pending.delete(response.id); }
  });
  async function call(method, params = {}) {
    const id = ++seq;
    const response = new Promise(resolve => pending.set(id, resolve));
    socket.send(JSON.stringify({id, method, params}));
    const result = await response;
    assert.equal(result.error, undefined);
    return result.result;
  }
  async function evaluate(expression) {
    const result = await call('Runtime.evaluate', {expression, awaitPromise: true, returnByValue: true});
    assert.equal(result.exceptionDetails, undefined, JSON.stringify(result.exceptionDetails));
    return result.result.value;
  }
  for (let i = 0; i < 100 && !(await evaluate('typeof renderChat === "function"')); i++) { await delay(100); }
  const settle = () => evaluate('new Promise(r => requestAnimationFrame(() => requestAnimationFrame(() => requestAnimationFrame(r))))');
  await evaluate(`
    $('main').classList.add('hasLive'); $('live').classList.add('show');
    store.room = {id:'test'};
    store.messages = Array.from({length:80}, (_,i) => ({id:'m'+i, author:{kind:'human'}, body:'Message '+i+'\\n'+('Variable height detail.\\n'.repeat(i%7+1))}));
    renderChat();
  `);
  await settle();
  assert.equal(await evaluate('chatAtBottom()'), true);
  await evaluate('chatBox.scrollTop = chatBox.scrollHeight / 2');
  await settle();
  await evaluate(`window.anchorNode = [...chatBox.children].find(n => n.getBoundingClientRect().bottom > chatBox.getBoundingClientRect().top); window.anchorY = anchorNode.getBoundingClientRect().top;`);
  await evaluate(`store.messages.push({id:'new',author:{kind:'human'},body:'Incoming\\n'.repeat(20)}); renderChat();`);
  await settle();
  assert.ok(await evaluate('Math.abs(anchorNode.getBoundingClientRect().top - anchorY) < 1'), 'arrival moved history');
  assert.equal(await evaluate('chatLatest.hidden'), false);
  await evaluate(`const previous = store.messages.find(m => m.id === anchorNode.previousElementSibling.dataset.messageId); previous.body += '\\nExpanded audit detail'.repeat(12); renderChat();`);
  await settle();
  assert.ok(await evaluate('Math.abs(anchorNode.getBoundingClientRect().top - anchorY) < 1'), 'resized message moved history');
  await evaluate(`window.replay = store.messages.slice(); applySnapshot({room:{id:'test'}, head:replay.length});`);
  await settle();
  assert.ok(await evaluate('Math.abs(anchorNode.getBoundingClientRect().top - anchorY) < 1'), 'snapshot cleared the reading anchor');
  await evaluate(`replay.forEach((message,i) => applyEnvelope({seq:i+1,event:{event:'message_posted',message}}));`);
  await settle();
  assert.ok(await evaluate('Math.abs(anchorNode.getBoundingClientRect().top - anchorY) < 1'), 'replay moved history');
  await evaluate('chatLatest.click()');
  await settle();
  assert.equal(await evaluate('chatAtBottom()'), true);
  // Queue an arrival and start a scrollbar drag before its animation frame.
  await evaluate(`store.messages.push({id:'drag',author:{kind:'human'},body:'Arrival'}); renderChat();
    chatBox.dispatchEvent(new PointerEvent('pointerdown', {bubbles:true}));
    chatBox.scrollTop -= 250; window.dragTop = chatBox.scrollTop;`);
  await settle();
  assert.equal(await evaluate('chatBox.scrollTop'), await evaluate('dragTop'), 'queued follow beat scrollbar drag');
  await evaluate(`window.dispatchEvent(new PointerEvent('pointerup'));`);
  await settle();
  assert.equal(await evaluate('chatFollowing'), false);
  console.log('PASS: desktop arrivals, dynamic height anchoring, jump to latest, and scrollbar interaction lock');
} finally {
  socket?.close();
  chrome.kill('SIGTERM');
  await new Promise(resolve => chrome.once('exit', resolve));
  await rm(profile, {recursive: true, force: true});
}
