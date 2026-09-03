// GPTBots desktop UI: attaches to the in-process room host's WebSocket and
// renders the board, roster and chat live. The host controls talk to the
// tauri shell when present (window.__TAURI__), and degrade gracefully when
// the same page is opened in a plain browser against a remote gptbots host.

const HOST = 'ws://127.0.0.1:8787/ws';
const $ = (id) => document.getElementById(id);

// ---- tiny store mirroring the server projections (like the Flutter app) --
const store = {
  room: null,
  agents: new Map(),
  tasks: new Map(),
  messages: [],
  phase: null,
};

function handleMessage(raw) {
  let msg;
  try { msg = JSON.parse(raw); } catch { return; }
  switch (msg.type) {
    case 'snapshot': return applySnapshot(msg);
    case 'event': return applyEnvelope(msg.envelope);
    case 'phase': return setPhase(msg.phase);
    case 'closed': return setPhase('completed');
    case 'error': return banner('lagging behind; resyncing…');
  }
}

function applySnapshot(snap) {
  const room = snap.room || {};
  store.room = room;
  store.agents.clear();
  for (const a of snap.roster || []) store.agents.set(a.id, a);
  store.tasks.clear();
  for (const t of snap.tasks || []) store.tasks.set(t.id, t);
  store.messages = [];
  store.phase = room.phase;
  renderAll();
}

function applyEnvelope(env) {
  const e = (env && env.event) || {};
  if (e.event === 'message_posted') {
    const m = e.message || {};
    store.messages.push(m);
    renderChat(true);
  } else if (e.event === 'task_created' && e.task) {
    store.tasks.set(e.task.id, e.task);
    renderBoard();
  } else if (e.event === 'task_transitioned' && e.task) {
    const t = store.tasks.get(e.task);
    if (t) { t.state = e.to; if (e.to === 'claimed') t.assignee = (e.transition && e.transition.by) || t.assignee; }
    renderBoard();
  } else if (e.event === 'agent_status_changed' && store.agents.has(e.agent)) {
    store.agents.get(e.agent).status = e.to;
    renderRoster();
  }
}

function setPhase(phase) { store.phase = phase; renderHeader(); }

// ---- rendering ----
function renderAll() { renderHeader(); renderRoster(); renderBoard(); renderChat(true); }

function renderHeader() {
  const spec = (store.room && store.room.spec) || {};
  $('roomName').textContent = spec.name || 'GPTBots';
  $('roomGoal').textContent = spec.goal || '';
  $('phaseChip').textContent = store.phase || '';
  const paused = store.phase === 'paused';
  $('pauseBtn').textContent = paused ? 'Resume' : 'Pause';
  $('pauseBtn').disabled = !store.room;
}

function agentHandle(id) {
  const a = store.agents.get(id);
  return a ? a.handle : (id || '').slice(0, 10);
}

function statusColor(s) {
  const colors = { working: '#e6c15a', idle: '#6fbf73', waiting: '#7a86e6', faulted: '#e6655a', offline: '#7c838c' };
  return colors[s] || '#7c838c';
}

function renderRoster() {
  const list = [...store.agents.values()].sort((a, b) => (a.roles && a.roles[0] === 'coordinator' ? -1 : 1));
  $('rosterList').innerHTML = list.map((a) =>
    '<li><span class="dot" style="background:' + statusColor(a.status) + '"></span>@' + esc(a.handle) + ' ' +
    '<span class="role">' + esc((a.roles || []).join('/')) + ' · ' + a.status + '</span></li>'
  ).join('');
}

function taskColor(state) {
  const colors = {
    claimed: '#2f6fed', started: '#2f6fed', in_progress: '#2f6fed',
    ready: '#7c838c', draft: '#7c838c',
    in_review: '#c2a04e', approved: '#6fbf73',
    submitted: '#6fbf73', delivered: '#6fbf73', done: '#6fbf73',
    blocked: '#e6655a', failed: '#e6655a',
  };
  return colors[state] || '#7c838c';
}

function renderBoard() {
  const order = ['claimed', 'started', 'in_progress', 'blocked', 'in_review', 'ready', 'draft', 'submitted', 'delivered', 'done', 'failed'];
  const tasks = [...store.tasks.values()].sort((a, b) => {
    const ra = order.indexOf(a.state), rb = order.indexOf(b.state);
    return (ra < 0 ? 99 : ra) - (rb < 0 ? 99 : rb) || a.title.localeCompare(b.title);
  });
  $('boardCards').innerHTML = tasks.length ? tasks.map((t) =>
    '<div class="card" style="border-left-color:' + taskColor(t.state) + '">' +
    '<div class="t">' + esc(t.title) + '</div>' +
    '<div class="meta">' + t.state + ' · rev ' + (t.revision || 0) +
    (t.assignee ? ' · ' + esc(agentHandle(t.assignee)) : '') + '</div></div>'
  ).join('') : '<div style="padding:12px;opacity:.6">empty board</div>';
}

function authorOf(m) {
  const a = m.author || {};
  if (a.kind === 'human') return 'you';
  return '@' + agentHandle(a.agent || a.id);
}

function isHuman(m) { const a = m.author || {}; return a.kind === 'human'; }

function renderChat(scroll) {
  const box = $('messages');
  if (!store.messages.length) { box.innerHTML = '<div style="opacity:.55">No messages yet</div>'; return; }
  box.innerHTML = store.messages.map((m) =>
    '<div class="msg ' + (isHuman(m) ? 'human' : '') + '">' +
    '<div class="who">' + esc(authorOf(m)) + '</div><div class="body">' + esc(m.body || '') + '</div></div>'
  ).join('');
  if (scroll) box.scrollTop = box.scrollHeight;
}

function esc(text) {
  const map = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' };
  return String(text).replace(/[&<>"']/g, (c) => map[c] || c);
}

function banner(text) { const b = $('banner'); b.textContent = text; b.classList.add('show'); }
function clearBanner() { $('banner').classList.remove('show'); }

// ---- transport ----
let ws = null;
function connect(url) {
  if (ws && ws.readyState <= 1) return;
  ws = new WebSocket(url);
  ws.onopen = () => { $('connChip').textContent = 'connected'; clearBanner(); };
  ws.onclose = () => { $('connChip').textContent = 'disconnected'; };
  ws.onmessage = (ev) => handleMessage(ev.data);
  ws.onerror = () => { $('connChip').textContent = 'error'; };
}

function send(obj) { if (ws && ws.readyState === 1) ws.send(JSON.stringify(obj)); }

// ---- host controls (tauri) ----
async function invoke(cmd, args) {
  if (window.__TAURI__) return window.__TAURI__.core.invoke(cmd, args || {});
  return null;
}

async function refreshHostStatus() {
  const info = await invoke('host_status');
  if (info) {
    $('hostSt').textContent = info.running ? 'running ' + info.config : 'not running';
    if (info.running) {
      $('roomPath').value = info.config || '';
      connect(info.ws_url);
    }
  }
}

$('startBtn').onclick = async () => {
  const path = $('roomPath').value.trim();
  if (!path) return banner('enter a room file path');
  $('hostSt').textContent = 'starting…';
  try {
    const info = await invoke('host_start', { config: path });
    if (info) {
      $('hostSt').textContent = 'running ' + (info.config || '');
      clearBanner();
      connect(info.ws_url);
    }
  } catch (err) {
    $('hostSt').textContent = 'start failed';
    banner('Start failed: ' + ((err && (err.message || err)) || 'unknown error'));
  }
};
$('stopBtn').onclick = async () => {
  await invoke('host_stop', {});
  if (ws) { ws.close(); ws = null; }
  $('hostSt').textContent = 'not running';
};
$('sendBtn').onclick = sendChat;
$('msgBox').addEventListener('keydown', (e) => {
  if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); sendChat(); }
});
function sendChat() {
  const body = $('msgBox').value.trim();
  if (!body) return;
  send({ type: 'send_message', author: 'You', body });
  $('msgBox').value = '';
}
$('pauseBtn').onclick = () => {
  send({ type: store.phase === 'paused' ? 'resume' : 'pause' });
};

// ---- boot ----
refreshHostStatus();
connect(HOST);
